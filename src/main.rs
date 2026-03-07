use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Manager, Peripheral};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

// Nordic UART Service (NUS) の UUID
const NUS_RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const NUS_TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Micro:bit BLE to WebSocket Bridge", long_about = None)]
struct Args {
    /// 接続先のMicro:bitの5文字の識別ID（例: zagic）
    /// 指定された場合は「BBC micro:bit [zagic]」を検索します。
    #[arg(short, long)]
    id: Option<String>,

    /// 接続先のMicro:bitデバイス名（部分一致）
    #[arg(short = 'n', long, default_value = "BBC micro:bit")]
    name: String,

    /// デバイス名の完全一致を要求する
    #[arg(short, long)]
    exact: bool,

    /// 接続先のMACアドレス（またはデバイスID）で特定する
    #[arg(short, long)]
    mac: Option<String>,

    /// ローカルのWebSocketサーバーのポート
    #[arg(short, long, default_value_t = 4000)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    println!("Starting microbridge...");
    println!("Target Device Name: {}", args.name);
    if let Some(id) = &args.id {
        println!("Target Device ID: {}", id);
    }
    println!("WebSocket Port: {}", args.port);

    run_bridge(args).await?;

    println!("Bridge task has terminated. Dropping resources...");
    
    // 全てのリソース（Peripheral, Adapter, Manager）がここで Drop されるため
    // Windows 側 (WinRT API) にデバイスの解放が行き渡るまでの完全なラグ期間を設ける
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    
    println!("Shutdown sequence complete. Exiting.");
    Ok(())
}

async fn run_bridge(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Ok(_) = tokio::signal::ctrl_c().await {
            println!("\nShutdown signal received. Initiating disconnect sequence...");
            let _ = shutdown_tx_clone.send(());
        }
    });

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let central = adapters.into_iter().next().ok_or("No Bluetooth adapters found")?;

    let mut current_peripheral = loop {
        println!("Starting BLE scan to detect device...");
        central.start_scan(ScanFilter::default()).await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        let _ = central.stop_scan().await;

        if let Some(peripheral) = find_target_peripheral(&central, &args).await? {
            break peripheral;
        }
        let target_desc = args.id.as_deref().unwrap_or(&args.name);
        println!("No device found matching '{}'. Retrying in 5 seconds...", target_desc);
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    };

    println!("Device found. Starting bridge task...");

    let mut shutdown_rx = shutdown_tx.subscribe();
    loop {
        let rx = shutdown_tx.subscribe();
        let result = connect_and_setup(&current_peripheral, args.clone(), rx).await;

        match result {
            Ok(_) => {
                println!("Bridge task completed cleanly (shutdown).");
                break;
            }
            Err(e) => {
                eprintln!("Disconnected or error: {}. Retrying in 5 seconds...", e);
            }
        }

        tokio::select! {
            _ = shutdown_rx.recv() => break,
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                println!("Refreshing device list before reconnection attempt...");
                let _ = central.start_scan(ScanFilter::default()).await;
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                let _ = central.stop_scan().await;

                if let Ok(Some(new_p)) = find_target_peripheral(&central, &args).await {
                    current_peripheral = new_p;
                }
            }
        }
    }

    Ok(())
}

async fn find_target_peripheral(
    central: &btleplug::platform::Adapter,
    args: &Args,
) -> Result<Option<Peripheral>, Box<dyn std::error::Error>> {
    let peripherals = central.peripherals().await?;

    for peripheral in peripherals {
        let mut is_match = false;
        
        if let Some(target_mac) = &args.mac {
            if peripheral.id().to_string() == *target_mac {
                is_match = true;
            }
        } else if let Some(target_id) = &args.id {
            let expected_name = format!("BBC micro:bit [{}]", target_id);
            if let Some(properties) = peripheral.properties().await? {
                if let Some(local_name) = properties.local_name {
                    if local_name == expected_name {
                        is_match = true;
                    }
                }
            }
        } else if let Some(properties) = peripheral.properties().await? {
            if let Some(local_name) = properties.local_name {
                if args.exact {
                    if local_name == args.name {
                        is_match = true;
                    }
                } else if local_name.contains(&args.name) {
                    is_match = true;
                }
            }
        }

        if is_match {
            return Ok(Some(peripheral));
        }
    }
    Ok(None)
}

async fn connect_and_setup(
    peripheral: &Peripheral,
    args: Args,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let props = peripheral.properties().await?.unwrap_or_default();
    let name = props.local_name.unwrap_or_else(|| "Unknown".to_string());
    
    println!("Attempting to connect to '{}'...", name);
    
    let mut connected = false;
    for attempt in 1..=3 {
        if peripheral.connect().await.is_ok() {
            connected = true;
            break;
        }
        eprintln!("Connection attempt {} failed.", attempt);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    
    if !connected {
        return Err("Failed to connect".into());
    }
 
    println!("Connected. Discovering services...");
    peripheral.discover_services().await?;

    let chars = peripheral.characteristics();
    let rx_char = chars.iter().find(|c| c.uuid == NUS_RX_CHARACTERISTIC_UUID).cloned().ok_or("RX not found")?;
    let tx_char = chars.iter().find(|c| c.uuid == NUS_TX_CHARACTERISTIC_UUID).cloned().ok_or("TX not found")?;

    peripheral.subscribe(&tx_char).await?;

    let bind_addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    println!("WebSocket listening on ws://{}", bind_addr);

    let (ble_tx, mut ble_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(32);

    let mut notification_stream = peripheral.notifications().await?;
    
    let ws_tx_clone = ws_tx.clone();
    let tx_uuid = tx_char.uuid;
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::mpsc::channel::<()>(1);

    // BLE通知処理: ストリームが終了したら切断とみなす
    let disconnect_tx_ble = disconnect_tx.clone();
    let ble_to_ws_task = tokio::spawn(async move {
        while let Some(data) = notification_stream.next().await {
            if data.uuid == tx_uuid {
                let msg = String::from_utf8_lossy(&data.value);
                println!("Received from BLE: {}", msg);
                let _ = ws_tx_clone.send(data.value);
            }
        }
        println!("Notification stream ended.");
        let _ = disconnect_tx_ble.send(()).await;
    });

    // BLE書き込み処理
    let peripheral_write = peripheral.clone();
    let rx_char_write = rx_char.clone();
    let disconnect_tx_ws = disconnect_tx.clone();
    let ws_to_ble_task = tokio::spawn(async move {
        while let Some(data) = ble_rx.recv().await {
            let msg = String::from_utf8_lossy(&data);
            println!("Sending to BLE: {}", msg);
            for chunk in data.chunks(20) {
                if let Err(_) = peripheral_write.write(&rx_char_write, chunk, btleplug::api::WriteType::WithoutResponse).await {
                    let _ = disconnect_tx_ws.send(()).await;
                    return;
                }
            }
        }
    });

    // 接続状態監視（ポリリング）
    let peripheral_monitor = peripheral.clone();
    let disconnect_tx_monitor = disconnect_tx.clone();
    let monitor_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            if let Ok(connected) = peripheral_monitor.is_connected().await {
                if !connected {
                    println!("Connection lost (polled).");
                    let _ = disconnect_tx_monitor.send(()).await;
                    break;
                }
            }
        }
    });

    let res = loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                println!("Shutting down...");
                break Ok(());
            }
            _ = disconnect_rx.recv() => {
                println!("Disconnection detected.");
                break Err("Disconnected".into());
            }
            accept_res = listener.accept() => {
                if let Ok((stream, addr)) = accept_res {
                    println!("Client connected: {}", addr);
                    let ws_tx_sub = ws_tx.clone();
                    let ble_tx_sub = ble_tx.clone();
                    tokio::spawn(async move {
                        if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                            let (mut write, mut read) = ws.split();
                            let mut ws_rx = ws_tx_sub.subscribe();
                            loop {
                                tokio::select! {
                                    msg = ws_rx.recv() => {
                                        if let Ok(data) = msg {
                                            if write.send(tokio_tungstenite::tungstenite::Message::Binary(data.into())).await.is_err() { break; }
                                        } else { break; }
                                    }
                                    msg = read.next() => {
                                        if let Some(Ok(m)) = msg {
                                            match m {
                                                tokio_tungstenite::tungstenite::Message::Binary(d) => { let _ = ble_tx_sub.send(d.to_vec()).await; }
                                                tokio_tungstenite::tungstenite::Message::Text(t) => { let _ = ble_tx_sub.send(t.as_bytes().to_vec()).await; }
                                                tokio_tungstenite::tungstenite::Message::Close(_) => break,
                                                _ => {}
                                            }
                                        } else { break; }
                                    }
                                }
                            }
                        }
                        println!("Client disconnected: {}", addr);
                    });
                }
            }
        }
    };

    ble_to_ws_task.abort();
    ws_to_ble_task.abort();
    monitor_task.abort();
    let _ = peripheral.disconnect().await;
    res
}

