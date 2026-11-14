# device-env 本地容器镜像（无 Docker Hub 时）

`device_matrix.py` 支持两类容器：

1. 官方 Rust 镜像（网络可达时）：`--docker-image rust:1.100-bookworm`；
2. 本地 scratch 镜像：把 Linux 下编译的 `device_adapt` / `device_env_check`
   连同 glibc loader/libs 打进一个最小镜像，`docker import` 即可，无需 pull。

## 构建本地镜像（Linux 或 WSL，x86_64）

```bash
# 1) 编译（target 可指向任意可写目录）
CARGO_TARGET_DIR=/tmp/trit-device-target CARGO_TERM_COLOR=never \
  cargo build --locked --release -p trit-gpu \
  --example device_adapt --example device_env_check

# 2) 组装最小 rootfs（只含二进制与运行时库）
STAGE=/tmp/trit-device-stage
rm -rf "$STAGE"
mkdir -p "$STAGE/app" "$STAGE/lib/x86_64-linux-gnu" "$STAGE/lib64"
cp /tmp/trit-device-target/release/examples/device_adapt "$STAGE/app/"
cp /tmp/trit-device-target/release/examples/device_env_check "$STAGE/app/"
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$STAGE/lib/x86_64-linux-gnu/"
cp -L /lib/x86_64-linux-gnu/libm.so.6 "$STAGE/lib/x86_64-linux-gnu/"
cp -L /lib/x86_64-linux-gnu/libgcc_s.so.1 "$STAGE/lib/x86_64-linux-gnu/"
cp -L /lib64/ld-linux-x86-64.so.2 "$STAGE/lib64/"
tar -C "$STAGE" -cf trit-device-env.tar .

# 3) 在 Docker 宿主机导入（Windows 用 Docker Desktop 的 docker.exe）
docker import --platform linux/amd64 trit-device-env.tar trit-device-env:local
```

导入后在仓库根目录删除临时 tar：`rm trit-device-env.tar`。

## 运行

```bash
# 两个容器配额做检测：2 CPU / 8 CPU
python scripts/device_matrix.py \
  --skip-docker \
  --docker-local-image trit-device-env:local \
  --docker-local-cpusets 0-1,0-7 \
  --out matrix.json
```

或手动：

```bash
docker run --rm --cpuset-cpus 0-1 trit-device-env:local /app/device_adapt --seed 7
docker run --rm --cpuset-cpus 0-7 trit-device-env:local /app/device_env_check
```

## 注意

- 该镜像没有 shell/rustc，`device_env_check` 的 rustc 字段为 `unknown`；
- 镜像只适用于同一架构/glibc 家族的 Linux 容器；跨发行版/ARM64 需各自重建；
- 网络可用时优先用官方 `rust:*` 镜像，脚本的 `--docker-image` 模式会
  自动 pull。
