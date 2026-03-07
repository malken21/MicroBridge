# microbridge

Micro:bit とローカルPC上のアプリケーション間で、Bluetooth Low Energy (BLE) と WebSocket を用いて双方向通信を中継（ブリッジ）するRust製のサイドカープロセスです。

## 特徴

- **BLE Nordic UART Service (NUS)** を使用し、Micro:bitのシリアル通信をWebSocketメッセージに変換します。
- OS（Windows等）ですでにペアリングされている Micro:bit を自動検出し、WebSocket ポートにルーティングします。
- 単一の Micro:bit との安定した通信に特化しており、切断時の自動再試行機能を備えています。

## アーキテクチャ

```text
[ Micro:bit ] <---(BLE NUS)---> [ microbridge ] <---(WS 4000)---> [ ユーザーアプリケーション ]
```

## 前提条件

1. **Rustのインストール**: `cargo` コマンドが使用できること
2. **Micro:bit側の準備**:
   - MakeCode等で「Bluetooth UART サービス」を有効化したプログラムを書き込んでおいてください。
   - **重要**: 使用する Micro:bit を、事前にPCのOS（Bluetooth設定画面）からペアリングしておいてください。ペアリングされていないデバイスは検出されません。

## ビルド

```bash
cargo build --release
```

## 使用方法

起動すると、指定したデバイス名（デフォルトは `BBC micro:bit`）を持つペアリング済みデバイスを検索し、接続します。

```bash
cargo run --release -- [OPTIONS]
```

### オプション

- `-i, --id <ID>`
  接続先のMicro:bitの5文字の識別ID（例: `zagic`）。指定された場合は「BBC micro:bit [zagic]」を完全一致で検索します。
- `-n, --name <NAME>`
  接続対象のMicro:bitのデバイス名に含まれる文字列（部分一致）。デフォルト: `BBC micro:bit`
- `-e, --exact`
  デバイス名（`--name`で指定した文字列）の完全一致を要求します。
- `-m, --mac <MAC>`
  接続先のMACアドレス（Windowsの場合は内部のデバイスID）で特定します。
- `-p, --port <PORT>`
  PC側（ユーザーアプリ側）と通信するWebSocketサーバのポート番号。デフォルト: `4000`

### 実行例

デフォルト設定での起動:

```bash
cargo run
```

ポートを指定して起動:

```bash
cargo run -- --port 5000
```

Micro:bit 用のWebSocketサーバは `ws://localhost:4000` で待ち受けを開始します。アプリ側からこのエンドポイントへ接続することで、双方向データ通信が可能になります。

## 通信のテスト

1. Micro:bitに、Bluetoothで受信した文字列をそのまま返す（またはLEDに表示する）プログラムを書き込みます。
2. 本Bridgeサーバーを起動します。
3. 別のターミナルから `wscat`（`npm install -g wscat`）等のWebSocket通信ツールを使用してテストします。

```bash
# アプリケーションの代わりに手動で接続と送信
wscat -c ws://localhost:4000
> Hello Microbit!
```

上記を入力してMicro:bit側が反応し、さらにMicro:bitからのデータが手元のターミナルへ届くことを確認してください。
