#!/usr/bin/env bash
# 构建安卓版前先加载这个环境：
#     source env.sh
#
# 每一项都可以按你自己的机器改。

# JDK 必须是 17 或 21。Gradle 8.x 遇到 JDK 22+ 会直接报错。
# 这里用的是机器上已有的 Microsoft OpenJDK 21（SJMCL 自带）。
if [ -d "/home/jx235/.local/share/SJMCL/runtime/java-21" ]; then
  export JAVA_HOME=/home/jx235/.local/share/SJMCL/runtime/java-21
else
  export JAVA_HOME=${JAVA_HOME:-/opt/java21}
fi

export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/sdk}
export NDK_HOME=${NDK_HOME:-$ANDROID_HOME/ndk/27.3.13750724}

# 关键：JDK 21 默认用 posix_spawn 派生子进程，但本机系统策略把它挡了
# （表现是 Gradle 报 "error=13, 权限不够"，连启动 /bin/echo 都失败）。
# 强制改用 fork 机制即可。实测：posix_spawn 失败，fork / vfork 成功。
# JAVA_TOOL_OPTIONS 对所有 JVM 生效，包括 Gradle 启动器、daemon 和它调用的
# aapt2 / d8 / zipalign 等子进程。
export JAVA_TOOL_OPTIONS="-Djdk.lang.Process.launchMechanism=fork"

export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$PATH"

echo "JAVA_HOME    = $JAVA_HOME"
echo "ANDROID_HOME = $ANDROID_HOME"
echo "NDK_HOME     = $NDK_HOME"
java -version 2>&1 | head -1
