運用・配布のための簡易ガイド

サーバー起動（PowerShell）
- サーバーPCでファイアウォール許可（初回のみ）:
  - New-NetFirewallRule -DisplayName "Bevy UDP 5000" -Direction Inbound -Protocol UDP -LocalPort 5000 -Action Allow
- 起動（LAN 例: 192.168.1.10:5000）:
  - .\run-server.ps1 -Address 192.168.1.10 -Port 5000 -LogLevel warn

クライアント起動（PowerShell）
- LAN のサーバーに接続（低負荷オプション例付き）:
  - .\run-client.ps1 -Server 192.168.1.10:5000 -LowGfx -NoVsync -LogLevel warn
- 同一PCで複数クライアントを動かす場合のみ、ローカルポート分離:
  - .\run-client.ps1 -Server 127.0.0.1:5000 -ClientPort 55001
  - .\run-client.ps1 -Server 127.0.0.1:5000 -ClientPort 55002

exe がある場合はそれを起動、無ければ cargo 実行に自動フォールバックします。

配布物
- サーバー: server(.exe) と assets/ フォルダ一式
- クライアント: bevy-online-campus(.exe) と assets/ フォルダ一式
- どちらも exe と同じ階層に assets/ を配置してください。

環境変数（実装済み）
- SERVER_ADDR: 接続先/広告先 host:port（例: 192.168.1.10:5000）
- CLIENT_PORT: クライアントのローカルUDPポート固定（同一PCで複数実行時に使用）
- LOW_GFX: 1 で影/HDRを無効化（低負荷モード）
- NO_VSYNC: 1 で VSync 無効
- RUST_LOG: ログ詳細度（warn を推奨）

WAN 運用のメモ
- VPS 上で server を常駐（systemd等）し、UDP/5000 を開放
- Secure 認証に対応（ENV でON/OFF）
  - 既定: Unsecure（ENV未設定）
  - Secureにする: `SECURE=1` と 32バイト鍵を指定
    - `NETCODE_KEY=<64桁HEX>` もしくは `NETCODE_KEY_FILE=<鍵ファイルパス>`（バイナリ32B or HEX文字列）
  - 例: `SECURE=1 NETCODE_KEY=0x001122...ffeedd SERVER_ADDR=0.0.0.0:5000 ./server`
  - クライアントも同じ鍵を設定: `SECURE=1 NETCODE_KEY=... SERVER_ADDR=example.com:5000 ./bevy-online-campus`
Linux 用スクリプト・常駐化
- サーバー起動（bash）:
  - 権限付与: `chmod +x ./run-server.sh`
  - 例: `./run-server.sh --address 0.0.0.0 --port 5000 --log warn --secure --key 0x<64HEX>`
  - キーファイル利用: `./run-server.sh --secure --key-file /opt/bevy/key.hex`
  - 既定で `WGPU_BACKEND=vk` と `WGPU_ALLOW_SOFTWARE=1` を設定（GPUなしVPS向け）
- クライアント起動（bash）:
  - 権限付与: `chmod +x ./run-client.sh`
  - 例: `./run-client.sh --server <IPまたはFQDN>:5000 --low-gfx --no-vsync --log warn`
  - Secure利用時: `./run-client.sh --server <IP>:5000 --secure --key 0x<64HEX>`

systemd 常駐（Linux）
- テンプレート: `systemd/bevy-server.service`
- 手順:
  1) `/opt/bevy` に `server` と `assets/` を配置（必要なら `key.hex` も）
  2) `sudo cp systemd/bevy-server.service /etc/systemd/system/`
  3) 必要に応じて `/etc/systemd/system/bevy-server.service` 内の Environment を編集
  4) `sudo systemctl daemon-reload && sudo systemctl enable --now bevy-server`
  5) ログ追尾: `journalctl -u bevy-server -f`
