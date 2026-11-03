chendownloader (Windows x64 portable; version in release.json)
===========================================================

中文使用说明（默认）
------------------
1. 支持 Windows 10 22H2 或 Windows 11（x86-64）。系统需安装 Microsoft Edge WebView2 Runtime。
2. 请先把 ZIP 完整解压到普通文件夹，再运行 chendownloader.exe。不要直接在压缩包预览窗口中运行。
3. 首次启动会把压缩包内已经校验过的 yt-dlp、Deno、gallery-dl、FFmpeg 和 FFprobe 复制到：
   %LOCALAPPDATA%\app.localmediadownloader.desktop\managed-tools
   媒体、链接、元数据与登录态不会上传到开发者服务器。
4. 本版本不会自动联网下载或更新任何依赖。组件损坏时，请使用设置中的“修复本地组件”；若仍失败，请重新下载并完整解压整个 ZIP。
5. 更新应用时，请下载新的完整 ZIP，核对 SHA-256 后解压到新文件夹。不要混合新旧版本文件。
6. 本版本未进行代码签名。Windows SmartScreen 可能显示“未知发布者”；请只从你信任的分发来源获取 ZIP，并先验证哈希。
7. Instagram 仅支持无需登录即可访问的公开帖子和 Reels；帖子可选择单项或全部下载，不提供剪切或手动清晰度选择。需要登录的内容不会绕过平台限制。
8. X 支持无需登录的公开帖子图片、视频和动图视频，可按单项／所选／整帖下载最高可用质量。不提供剪切、手动清晰度或音频格式选择；默认保真不转码，只有主动开启兼容模式才转换。登录受限、直播等未支持内容不绕过限制。
9. YouTube、Instagram、X 和 Bilibili 共用下载任务队列，默认同时执行 2 个任务，可在设置中调整为 1–4 个。提交后可继续解析其他链接或切换平台；各任务可独立取消。退出应用会取消未结束任务，本版本不支持跨重启续传。
10. Bilibili 支持普通 BV/av 视频及指定的单个 P，也可粘贴末尾带唯一普通视频链接的分享文案。支持最高质量、手动、仅视频、仅音频、可选兼容模式和本地剪切。番剧／会员／付费试看、短链接、多 P 批量、直播、课程与互动视频尚未支持。
11. 默认保留原始最高可用音视频流，按编码兼容性无损封装为 MP4、WebM 或 MKV。只有主动开启兼容模式才允许转码。真实平台可用性受地区、系统代理和匿名访问限制影响；缺失项保留原序号，不把部分成功标为整帖成功。

校验 ZIP：
  PowerShell: (Get-FileHash .\chendownloader-<版本>-windows-x64.zip -Algorithm SHA256).Hash
请将 <版本> 替换为下载压缩包的实际版本号。
将结果与同目录 .zip.sha256 文件中的小写十六进制值比较。

隐私与登录态
------------
- 默认不读取浏览器 Cookie，不要求账户，也不设置云端账户。
- 后续如支持登录态导入，必须由用户主动选择并只保存在本机；当前便携版不包含此功能。
- 下载流量由用户电脑直接访问目标平台，不经过本项目服务器。

English quick start
-------------------
Extract the whole ZIP before running chendownloader.exe. Windows 10 22H2/Windows 11 x64 and Microsoft Edge WebView2 Runtime are required. The five bundled tools are verified and copied to the app's LocalAppData directory on first launch. No dependency is downloaded or updated automatically. Instagram support is limited to public posts and Reels available without sign-in; posts support individual or batch download, without clipping or manual quality selection. Updates are performed by downloading, hashing, and extracting a complete new ZIP. This unsigned build may trigger a SmartScreen “Unknown publisher” warning.

X public status attachments support images, videos and silent animated videos, with highest-quality individual/selected/whole-post download. X has no clipping or manual format controls. Original streams are copied by default; only explicitly enabled compatibility mode permits conversion. Login-only/private/live content is not bypassed; partial results are not labelled whole-post success.

YouTube, Instagram, X and Bilibili share a download queue with two concurrent tasks by default, configurable from one to four in Settings. After submission, you can parse another link or switch platforms without stopping existing downloads. Tasks can be cancelled individually. Exiting the app cancels unfinished tasks; resuming across app restarts is not supported.

Bilibili supports ordinary BV/av videos and one selected P, including share text ending in one ordinary-video URL. Four download modes, optional compatibility and local clipping are available. Bangumi/member/paid previews, short links, multi-P batches, live, courses and interactive videos are not supported. Default output copies highest-available original streams into MP4, WebM or MKV according to codec compatibility, without downgrading to force a container. Site availability depends on the user's network and anonymous access.

Licenses and source material are in THIRD_PARTY_NOTICES.md and the licenses directory.
