# microbridge

Micro:bit とローカルPC上のアプリケーション間で、Bluetooth Low Energy (BLE) と WebSocket を用いて双方向通信を中継（ブリッジ）するRust製のサイドカープロセスです。

## 特徴

- **BLE Nordic UART Service (NUS)** を使用し、Micro:bitのシリアル通信をWebSocketメッセージに変換します。
- **複数台のMicro:bit同時接続**に対応。OS（Windows等）ですでにペアリングされている複数のMicro:bitを自動検出し、それぞれ独立したWebSocketポートにルーティングします。
- 検出されたデバイスごとに、指定したベースのWebSocketポートから自動でインクリメント（+1）してポートを割り当てます。

## アーキテクチャ

```text
[ Micro:bit #1 ] <---(BLE NUS)---> [ microbridge ] <---(WS 4000)---> [ ユーザーアプリケーション ]
[ Micro:bit #2 ] <---(BLE NUS)---> [ microbridge ] <---(WS 4001)---> [ ユーザーアプリケーション ]
```

## 前提条件

1. **Rustのインストール**: `cargo` コマンドが使用できること
2. **Micro:bit側の準備**:
   - MakeCode等で「Bluetooth UART サービス」を有効化したプログラムを書き込んでおいてください。
   - **重要**: 使用するすべてのMicro:bitを、事前にPCのOS（Bluetooth設定画面）からペアリングしておいてください。ペアリングされていないデバイスは検出されません。

## ビルド

```bash
cargo build --release
```

## 使用方法

起動すると、指定したデバイス名（デフォルトは `BBC micro:bit`）を持つペアリング済みデバイスを検索し、順次接続します。

```bash
cargo run --release -- [OPTIONS]
```

### オプション

- `-d, --device-name <DEVICE_NAME>`
  接続対象のMicro:bitのデバイス名に含まれる文字列。デフォルト: `BBC micro:bit`
- `-p, --port <PORT>`
  PC側（ユーザーアプリ側）と通信するWebSocketサーバのベースポート番号。デフォルト: `4000`

### 実行例

デフォルト設定での起動:

```bash
cargo run
```

ベースポートを変更して起動:

```bash
cargo run -- --port 5000
```

1台目のMicro:bit用のWebSocketサーバは `ws://localhost:5000` で待ち受けを開始します。アプリ側からこのエンドポイントへ接続することで、双方向データ通信が可能になります。
2台目のMicro:bitはそれぞれ `ws://localhost:5001` と連番になります。

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
