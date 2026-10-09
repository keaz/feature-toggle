# FluxGate release archives

Each tagged release on GitHub has one archive per component and platform. The binaries are built from the `feature-toggle-backend`, `feature-edge-server` and `fluxgate-cli` crates and renamed:

| Archive | Contents |
|---|---|
| `fluxgate-backend-<version>-<platform>.tar.gz` | `fluxgate-backend`, `config.toml`, `log4rs.yaml`, this README |
| `fluxgate-edge-<version>-<platform>.tar.gz` | `fluxgate-edge`, `config.toml`, `log4rs.yaml`, `CONFIG.md` |
| `fluxgate-<version>-<platform>.tar.gz` | the `fluxgate` CLI, `README.md` |

Platforms: `linux-x86_64`, `linux-arm64`, `macos-arm64`, `macos-x86_64`. `SHA256SUMS` lists the checksum of every archive. The Linux builds need glibc 2.35 or later (Ubuntu 22.04, Debian 12, RHEL 9) and OpenSSL 3.

The files in this folder are the templates packed into the archives.

## Backend

The backend needs PostgreSQL 15 or later and an empty database. It runs its migrations at startup.

```bash
mkdir fluxgate-backend && tar -xzf fluxgate-backend-<version>-<platform>.tar.gz -C fluxgate-backend
cd fluxgate-backend
export DATABASE_URL=postgres://postgres:<password>@localhost:5432/feature_toggle
[ -f encryption.key ] || openssl rand -base64 32 > encryption.key
export FLUXGATE_ENCRYPTION_KEY=$(cat encryption.key)
./fluxgate-backend
```

Keep `encryption.key`. The backend encrypts stored secrets with it, and they cannot be decrypted with a new key.

The backend reads `config.toml` and `log4rs.yaml` from the working directory. Set `FEATURE_TOGGLE_CONFIG` to use another config file.

## Edge server

```bash
mkdir fluxgate-edge && tar -xzf fluxgate-edge-<version>-<platform>.tar.gz -C fluxgate-edge
cd fluxgate-edge
export EDGE_BACKEND_GRPC=http://localhost:50051
export EDGE_CLIENT_ID=<client id>
export EDGE_CLIENT_SECRET=<client secret>
./fluxgate-edge
```

The edge reads `config.toml` and `log4rs.yaml` from the working directory. `EDGE_*` environment variables override the file; see `CONFIG.md`.

## macOS

The binaries are not signed. If macOS blocks one, remove the quarantine attribute after you unpack it:

```bash
xattr -d com.apple.quarantine ./fluxgate-backend
```
