# MicroBridge

Micro:bit とローカルPC上のアプリケーション間で、Bluetooth Low Energy (BLE) と UDP を用いて双方向通信を中継（ブリッジ）するRust製のサイドカープロセスです。

## 特徴

- **BLE Nordic UART Service (NUS)** を使用し、Micro:bitのシリアル通信をUDPパケットに変換します。
- **複数台のMicro:bit同時接続**に対応。OS（Windows等）ですでにペアリングされている複数のMicro:bitを自動検出し、それぞれ独立したUDPポートにルーティングします。
- 検出されたデバイスごとに、指定したベースのUDPポートから自動でインクリメント（+1）してポートを割り当てます。

## アーキテクチャ

```
[ Micro:bit #1 ] <---(BLE NUS)---> [ MicroBridge ] <---(UDP 4000/5000)---> [ ユーザーアプリケーション ]
[ Micro:bit #2 ] <---(BLE NUS)---> [ MicroBridge ] <---(UDP 4001/5001)---> [ ユーザーアプリケーション ]
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
- `-b, --bind-port <BIND_PORT>`
  PC側（ユーザーアプリ側）からのUDPパケットを受信するベースポート番号。デフォルト: `4000`
- `--dest-port <DEST_PORT>`
  PC側（ユーザーアプリ側）へUDPパケットを送信する宛先のベースポート番号。デフォルト: `5000`

### 実行例

デフォルト設定での起動:

```bash
cargo run
```

ベースポートを変更して起動:

```bash
cargo run -- --bind-port 5000 --dest-port 6000
```

1台目のMicro:bitからのデータは `localhost:6000` に送信され、アプリ側から `localhost:5000` にUDPでデータを送ると1台目のMicro:bitに転送されます。
2台目のMicro:bitはそれぞれ `6001`, `5001` と連番になります。

## 通信のテスト

1. Micro:bitに、Bluetoothで受信した文字列をそのまま返す（またはLEDに表示する）プログラムを書き込みます。
2. 本Bridgeサーバーを起動します。
3. 別のターミナルから `nc`（netcat）等のUDP通信ツールを使用してテストします。

```bash
# アプリケーションの代わりに手動でUDPパケットを送信
nc -u 127.0.0.1 4000
> Hello Microbit!
```

上記を入力してMicro:bit側が反応し、さらに `nc -ul 5000` などの受信側ポートにデータが届くことを確認してください（複数デバイスの場合はポート 4001, 5001 なども合わせて確認）。
