#!/usr/bin/env bash
# 给已打包好的 APK 签名（不用重新跑 Gradle）。
# 用法：./sign-apk.sh
set -e
cd "$(dirname "$0")"

# 这两行是关键：JDK 21 默认用 posix_spawn 派生子进程，本机被系统策略挡住，
# 强制改用 fork 之后 keytool / apksigner 这类 JVM 程序才能启动子进程。
export JAVA_HOME=/home/jx235/.local/share/SJMCL/runtime/java-21
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/sdk}
export JAVA_TOOL_OPTIONS="-Djdk.lang.Process.launchMechanism=fork"
export PATH="$JAVA_HOME/bin:$PATH"

UNSIGNED=$(find src-tauri/gen/android/app/build/outputs -name "*unsigned.apk" 2>/dev/null | head -1)
if [ -z "$UNSIGNED" ]; then
  echo "没找到未签名的 APK，先跑 ./build-android.sh"
  exit 1
fi
echo "待签名: $UNSIGNED"

KS=coachsource.jks
PASS=coachsource
if [ ! -f "$KS" ]; then
  echo "→ 生成自签名密钥 $KS（仅用于自己安装，不上架）"
  keytool -genkeypair -v -keystore "$KS" -alias coachsource -keyalg RSA -keysize 2048 \
    -validity 10000 -storetype JKS \
    -storepass "$PASS" -keypass "$PASS" \
    -dname "CN=CoachSource, OU=Personal, O=Personal, L=X, ST=X, C=CN" 2>&1 | grep -v "Picked up" || true
fi

BT=$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -1)
BT=${BT%/}
echo "→ 对齐 + 签名（$BT）"
"$BT/zipalign" -f -p 4 "$UNSIGNED" app-aligned.apk
"$BT/apksigner" sign --ks "$KS" --ks-key-alias coachsource \
  --ks-pass "pass:$PASS" --key-pass "pass:$PASS" \
  --out app-signed.apk app-aligned.apk 2>&1 | grep -v "Picked up" || true
"$BT/apksigner" verify app-signed.apk 2>&1 | grep -v "Picked up" || true

echo
echo "✓ 完成：$(pwd)/app-signed.apk  ($(du -h app-signed.apk | cut -f1))"
echo "  安装：adb install -r app-signed.apk"
