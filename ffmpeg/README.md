# ffmpeg 运行时目录

本目录在仓库中仅作占位。GitHub Actions 构建 Windows 安装包时，
会自动下载 Windows ffmpeg essentials 构建并把 `ffmpeg.exe` 放入此处，
随后由 Tauri 作为资源捆绑进安装包，使录制/转码开箱即用、无需用户另行安装。

CI 下载步骤见 `.github/workflows/build.yml` 的 "Bundle ffmpeg (Windows)"。
