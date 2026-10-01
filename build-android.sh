#!/usr/bin/env bash
# 打包安卓 APK。在你自己的终端里运行：
#     ./build-android.sh
#
# Gradle 全靠 JVM 派生子进程干活（启动 daemon、调 aapt2 / d8 / zipalign）。
# JDK 21 默认用 posix_spawn，本机系统策略把它挡了 —— 表现为
#     Could not start Gradle build daemon ... error=13, 权限不够
# env.sh 里已经把启动机制强制改成 fork，加载 env.sh 就能绕过。
#
# Rust 那部分通常已经编译好了，所以这一步主要就是 Gradle 打包，几分钟。

set -e
cd "$(dirname "$0")"

# 加载 JAVA_HOME / ANDROID_HOME / NDK_HOME
source ./env.sh

# Gradle 官方源在国内不通，这里确保用镜像（已经改过的话这行是幂等的）
WRAPPER=src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties
if [ -f "$WRAPPER" ] && grep -q "services.gradle.org" "$WRAPPER"; then
  echo "→ 切换 Gradle 下载源到腾讯云镜像"
  sed -i 's|distributionUrl=.*|distributionUrl=https\\://mirrors.cloud.tencent.com/gradle/gradle-9.6.1-bin.zip|' "$WRAPPER"
fi

echo "→ 开始打包 APK（首次约 10-20 分钟）"
pnpm android:build

echo "→ 找 APK："
UNSIGNED=$(find src-tauri/gen/android/app/build/outputs -name "*unsigned.apk" 2>/dev/null | head -1)
echo "  $UNSIGNED"

# Gradle 产出的 release 包是未签名的，手机上装不上，这里自己签一个。
# 自签名只用于自己安装，不上架应用商店，所以口令写死在脚本里无妨。
KS=coachsource.jks
STOREPASS=coachsource
if [ ! -f "$KS" ]; then
  echo "→ 第一次打包，生成签名密钥 $KS"
  keytool -genkeypair -v -keystore "$KS" -alias coachsource -keyalg RSA -keysize 2048 \
    -validity 10000 -storetype JKS \
    -storepass "$STOREPASS" -keypass "$STOREPASS" \
    -dname "CN=CoachSource, OU=Personal, O=Personal, L=X, ST=X, C=CN"
fi

BT=$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -1)
BT=${BT%/}
echo "→ 用 $BT 对齐并签名"
"$BT/zipalign" -f -p 4 "$UNSIGNED" app-aligned.apk
"$BT/apksigner" sign --ks "$KS" --ks-key-alias coachsource \
  --ks-pass "pass:$STOREPASS" --key-pass "pass:$STOREPASS" \
  --out app-signed.apk app-aligned.apk
"$BT/apksigner" verify app-signed.apk

echo
echo "✓ 已签名的安装包："
echo "  $(pwd)/app-signed.apk"
echo "  $(du -h app-signed.apk | cut -f1)"
echo
echo "手机开 USB 调试连上电脑后安装："
echo "  adb install -r app-signed.apk"
