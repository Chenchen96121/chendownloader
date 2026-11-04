# 源码构建

## 环境

- Windows x64、Node.js 24 和 npm。
- Rust/MSVC；固定版本见 `rust-toolchain.toml`。
- Visual Studio C++ 桌面工具链及 Windows SDK。
- Microsoft Edge WebView2 Runtime。

## 桌面开发

```powershell
npm ci
npm run tauri -- dev
```

首次运行需要受信本地组件。可以先运行完整便携包，让应用在 LocalAppData 中准备同版本组件；也可以将便携包内 `resources/managed-tools` 整个目录放到 `src-tauri/target/debug/resources/managed-tools`。组件摘要必须与 `src-tauri/resources/managed-tools/manifest.json` 一致。

## TikTok 本地组件

使用独立 Python 3.12.14 环境，按 `tools/tiktok-core/requirements.lock` 的哈希安装依赖，再执行：

```powershell
./scripts/build-tiktok-core.ps1 -Python <隔离环境的python.exe>
```

脚本先测试 onedir，再构建 onefile，并将真实摘要写入 `src-tauri/resources/tiktok-core/manifest.json`。不要手工填写摘要。普通用户只需完整便携包，无需 Python。开发时辅助程序放在应用 EXE 旁的 `resources/platform-tools/` 中，与五个通用组件分开校验。来源和许可证见 `tools/tiktok-core/README.md`。

## 构建便携包

将五个 `.exe` 组件单独放入源码目录之外的普通文件夹，文件名和摘要须匹配受信清单。该输入文件夹不要附加 `manifest.json` 或其他文件。gallery-dl 对应源码可从便携包的 `licenses/sources/gallery-dl-1.32.15.tar.gz` 取得。

```powershell
npm run package:portable -- -VendorRoot <五个组件所在的绝对路径> -GalleryDlSourceArchive <gallery-dl源码归档的绝对路径> -TikTokCorePath <TikTok辅助程序绝对路径>
```

输出位于 `.release/`，包含完整便携 ZIP 及 SHA-256 文件。运行组件来自本地输入，不会在应用启动时联网下载安装。

FFmpeg/FFprobe 的源码、固定编译镜像、离线配方和组件许可在 Release 对应源码附件中提供。其 `README.md` 说明源码校验和重建步骤。

## 源码检查

```powershell
npm test
npm run test:packaging
cd src-tauri
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

普通源码测试使用合成数据；需要真实工具或网络的测试通过显式配置启用。构建缓存、测试输出、运行数据和本地配置均不属于源代码发布内容。
