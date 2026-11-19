# LAN Share

Share files between a Windows PC and Android phones over wifi. Any device can host; others connect with its IP and PIN.

- `shared/index.html`: the UI, used by both apps
- `windows/`: Tauri app (Rust server + window)
- `android/`: Java app (NanoHTTPD server + WebView)
- `.github/workflows/build.yml`: builds both on every push

## Use
1. Install and open the app on both devices (same wifi). Each shows its **IP** and **PIN** at the top.
2. On one device, type the other's IP and PIN, press **Connect**. Both devices can now see each other.
3. Left pane = this device, right pane = the other device. Tick files or folders, then **Send** or **Download**. Click a folder's name to open it; folders copy with everything inside, and a progress bar shows the transfer.

Windows: allow the app through the firewall on first run (private networks).
Android: grant "All files access" when asked.
