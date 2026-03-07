use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Manager, Peripheral};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

// Nordic UART Service (NUS) の UUID
const NUS_RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
const NUS_TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Micro:bit BLE to UDP Bridge", long_about = None)]
struct Args {
    /// 接続先のMicro:bitデバイス名
    #[arg(short, long, default_value = "BBC micro:bit")]
    device_name: String,

    /// ローカルのWebSocketサーバーのポート
    #[arg(short, long, default_value_t = 4000)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    println!("Starting microbridge (Multi-Device)...");
    println!("Target Device Name: {}", args.device_name);
    println!("WebSocket Base Port: {}", args.port);

    run_bridge(args).await?;

    println!("All bridge tasks have terminated. Dropping resources...");
    
    // 全てのリソース（Peripheral, Adapter, Manager）がここで Drop されるため
    // Windows 側 (WinRT API) にデバイスの解放が行き渡るまでの完全なラグ期間を設ける
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    
    println!("Shutdown sequence complete. Exiting.");
    Ok(())
}

async fn run_bridge(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;

    let central = adapters
        .into_iter()
        .next()
        .ok_or("No Bluetooth adapters found")?;

    println!("Starting BLE scan to refresh device list...");
    central.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    let _ = central.stop_scan().await;

    let peripherals = find_target_peripherals(&central, &args.device_name).await?;

    if peripherals.is_empty() {
        eprintln!("No devices matching '{}' were found. Please ensure they are paired and powered on.", args.device_name);
        return Ok(());
    }

    println!("Found {} matching peripheral(s).", peripherals.len());

    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Ok(_) = tokio::signal::ctrl_c().await {
            println!("\nShutdown signal received. Initiating disconnect sequence...");
            let _ = shutdown_tx_clone.send(());
        }
    });

    let mut tasks = Vec::new();

    for (index, peripheral) in peripherals.into_iter().enumerate() {
        let p_args = args.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        let task = tokio::spawn(async move {
            if let Err(e) = connect_and_setup(&peripheral, p_args, index as u16, shutdown_rx).await {
                eprintln!("Task {} failed: {}", index, e);
            }
        });
        tasks.push(task);
    }

    // すべてのブリッジタスクの完了を待機
    for task in tasks {
        let _ = task.await;
    }

    Ok(())
}

async fn find_target_peripherals(
    central: &btleplug::platform::Adapter,
    target_name: &str,
) -> Result<Vec<Peripheral>, Box<dyn std::error::Error>> {
    let mut matched = Vec::new();
    let peripherals = central.peripherals().await?;

    for peripheral in peripherals {
        if let Some(properties) = peripheral.properties().await? {
            if let Some(local_name) = properties.local_name {
                if local_name.contains(target_name) {
                    matched.push(peripheral);
                }
            }
        }
    }
    Ok(matched)
}

async fn connect_and_setup(
    peripheral: &Peripheral,
    args: Args,
    index: u16,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ws_port = args.port + index;
    
    let props = peripheral.properties().await?.unwrap_or_default();
    let name = props.local_name.unwrap_or_else(|| "Unknown".to_string());
    
    println!("[{}] Connecting to '{}'...", index, name);
    
    // OSレベルですぐに失敗した場合は、接続を何度か再試行する
    let mut connected = false;
    for attempt in 1..=3 {
        if peripheral.connect().await.is_ok() {
            connected = true;
            break;
        }
        eprintln!("[{}] Connection attempt {} failed, retrying...", index, attempt);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    
    if !connected {
        return Err("Failed to connect after multiple attempts".into());
    }

    println!("[{}] Discovering services...", index);
    if let Err(e) = peripheral.discover_services().await {
        return Err(format!("Failed to discover services: {}", e).into());
    }

    let chars = peripheral.characteristics();
    let rx_char = chars.iter().find(|c| c.uuid == NUS_RX_CHARACTERISTIC_UUID);
    let tx_char = chars.iter().find(|c| c.uuid == NUS_TX_CHARACTERISTIC_UUID);

    if rx_char.is_none() || tx_char.is_none() {
        eprintln!("[{}] Nordic UART Service characteristics not found.", index);
        return Err("Characteristics not found".into());
    }

    let rx_char = rx_char.unwrap().clone();
    let tx_char = tx_char.unwrap().clone();

    println!("[{}] Subscribing to TX characteristic...", index);
    if let Err(e) = peripheral.subscribe(&tx_char).await {
         return Err(format!("Failed to subscribe to TX: {}", e).into());
    }

    let bind_addr = format!("0.0.0.0:{}", ws_port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    println!("[{}] WebSocket server listening on ws://{}", index, bind_addr);

    let (ble_tx, mut ble_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(32);

    let mut notification_stream = peripheral.notifications().await?;
    let ws_tx_clone = ws_tx.clone();
    let tx_uuid = tx_char.uuid;

    let (disconnect_tx, mut disconnect_rx) = tokio::sync::mpsc::channel::<()>(1);
    let disconnect_tx_clone1 = disconnect_tx.clone();
    let disconnect_tx_clone2 = disconnect_tx.clone();

    // BLEの通知を読み取り、WebSocketクライアントにブロードキャストするタスク
    let ble_to_ws_task = tokio::spawn(async move {
        while let Some(data) = notification_stream.next().await {
            if data.uuid == tx_uuid {
                let msg = String::from_utf8_lossy(&data.value);
                println!("[{}] Received from BLE: {}", index, msg);
                let _ = ws_tx_clone.send(data.value);
            }
        }
        println!("[{}] BLE notification stream ended. Peripheral might be disconnected.", index);
        let _ = disconnect_tx_clone1.send(()).await;
    });

    let peripheral_clone = peripheral.clone();
    let rx_char_clone = rx_char.clone();
    // mpscから読み取り、BLEに書き込むタスク
    let ws_to_ble_task = tokio::spawn(async move {
        while let Some(data) = ble_rx.recv().await {
            let msg = String::from_utf8_lossy(&data);
            println!("[{}] Sending to BLE: {}", index, msg);
            // MTU制限超過による書き込みエラーを回避するため、20バイトごとに分割して送信
            for chunk in data.chunks(20) {
                if let Err(e) = peripheral_clone
                    .write(&rx_char_clone, chunk, btleplug::api::WriteType::WithoutResponse)
                    .await
                {
                    eprintln!("[{}] Failed to write chunk to BLE: {}", index, e);
                    let _ = disconnect_tx_clone2.send(()).await;
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                println!("[{}] Received shutdown signal. Disconnecting peripheral...", index);
                
                // サブタスク内で保持されているPeripheralやCharacteristicの参照を完全に破棄するため、タスクを強制終了して完了を待機
                ble_to_ws_task.abort();
                ws_to_ble_task.abort();
                let _ = ble_to_ws_task.await;
                let _ = ws_to_ble_task.await;

                println!("[{}] Unsubscribing from TX characteristic...", index);
                let _ = peripheral.unsubscribe(&tx_char).await;

                let _ = peripheral.disconnect().await;
                break;
            }
            _ = disconnect_rx.recv() => {
                eprintln!("[{}] Peripheral disconnected or unexpected error occurred. Exiting task...", index);
                break;
            }
            accept_res = listener.accept() => {
                if let Ok((stream, addr)) = accept_res {
                    println!("[{}] Client connected: {}", index, addr);
                    let ws_stream = tokio_tungstenite::accept_async(stream).await;
                    match ws_stream {
                        Ok(ws) => {
                            let (mut write, mut read) = ws.split();
                            let mut ws_rx = ws_tx.subscribe();
                            let ble_tx_clone = ble_tx.clone();

                            tokio::spawn(async move {
                                loop {
                                    tokio::select! {
                                        msg = ws_rx.recv() => {
                                            match msg {
                                                Ok(data) => {
                                                    if write.send(tokio_tungstenite::tungstenite::Message::Binary(data.into())).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                                    eprintln!("[{}] WebSocket client lagged, skipped {} messages.", index, skipped);
                                                }
                                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                            }
                                        }
                                        msg = read.next() => {
                                            match msg {
                                                Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(data))) => {
                                                    let _ = ble_tx_clone.send(data.to_vec()).await;
                                                }
                                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(data))) => {
                                                    let _ = ble_tx_clone.send(data.as_str().as_bytes().to_vec()).await;
                                                }
                                                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                println!("[{}] Client disconnected: {}", index, addr);
                            });
                        }
                        Err(e) => {
                            eprintln!("[{}] WebSocket handshake failed: {}", index, e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
