use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Manager, Peripheral};
use clap::Parser;
use futures::stream::StreamExt;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{error, info, warn};
use uuid::Uuid;

// Nordic UART Service (NUS) UUIDs
const NUS_SERVICE_UUID: Uuid = Uuid::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);
const NUS_RX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
const NUS_TX_CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Micro:bit BLE to UDP Bridge", long_about = None)]
struct Args {
    /// Name of the Micro:bit device(s) to connect to
    #[arg(short, long, default_value = "BBC micro:bit")]
    device_name: String,

    /// Local UDP base port to bind

    #[arg(short, long, default_value_t = 4000)]
    bind_port: u16,

    /// Destination UDP base port to send Micro:bit data to
    #[arg(long, default_value_t = 5000)]
    dest_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    info!("Starting MicroBridge-Server (Multi-Device)...");
    info!("Target Device Name: {}", args.device_name);
    info!("Base UDP Bind Port: {}", args.bind_port);
    info!("Base UDP Dest Port: {}", args.dest_port);

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;

    let central = adapters
        .into_iter()
        .next()
        .ok_or("No Bluetooth adapters found")?;

    // Even though we want OS-paired devices, starting a short scan helps update the OS peripheral cache.
    info!("Starting BLE scan to refresh device list...");
    central.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let peripherals = find_target_peripherals(&central, &args.device_name).await?;

    if peripherals.is_empty() {
        warn!("No devices matching '{}' were found. Please ensure they are paired and powered on.", args.device_name);
        return Ok(());
    }

    info!("Found {} matching peripheral(s).", peripherals.len());

    let mut tasks = Vec::new();

    for (index, peripheral) in peripherals.into_iter().enumerate() {
        let p_args = args.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = connect_and_setup(&peripheral, p_args, index as u16).await {
                error!("Task {} failed: {}", index, e);
            }
        });
        tasks.push(task);
    }

    // Wait for all bridge tasks
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
) -> Result<(), Box<dyn std::error::Error>> {
    let bind_port = args.bind_port + index;
    let dest_port = args.dest_port + index;
    
    let props = peripheral.properties().await?.unwrap_or_default();
    let name = props.local_name.unwrap_or_else(|| "Unknown".to_string());
    
    info!("[{}] Connecting to '{}'...", index, name);
    
    // We retry connection a few times if it fails immediately in OS
    let mut connected = false;
    for attempt in 1..=3 {
        if peripheral.connect().await.is_ok() {
            connected = true;
            break;
        }
        warn!("[{}] Connection attempt {} failed, retrying...", index, attempt);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    
    if !connected {
        return Err("Failed to connect after multiple attempts".into());
    }

    info!("[{}] Discovering services...", index);
    if let Err(e) = peripheral.discover_services().await {
        return Err(format!("Failed to discover services: {}", e).into());
    }

    let chars = peripheral.characteristics();
    let rx_char = chars.iter().find(|c| c.uuid == NUS_RX_CHARACTERISTIC_UUID);
    let tx_char = chars.iter().find(|c| c.uuid == NUS_TX_CHARACTERISTIC_UUID);

    if rx_char.is_none() || tx_char.is_none() {
        error!("[{}] Nordic UART Service characteristics not found.", index);
        return Err("Characteristics not found".into());
    }

    let rx_char = rx_char.unwrap().clone();
    let tx_char = tx_char.unwrap().clone();

    info!("[{}] Subscribing to TX characteristic...", index);
    if let Err(e) = peripheral.subscribe(&tx_char).await {
         return Err(format!("Failed to subscribe to TX: {}", e).into());
    }

    let bind_addr = format!("127.0.0.1:{}", bind_port);
    let dest_addr = format!("127.0.0.1:{}", dest_port);

    let udp_socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
    info!("[{}] UDP Socket bound to {}", index, bind_addr);

    let mut notification_stream = peripheral.notifications().await?;

    // Relay BLE -> UDP
    let ble_to_udp_socket = udp_socket.clone();
    let dest_addr_clone = dest_addr.clone();
    let ble_to_udp_task = tokio::spawn(async move {
        info!("[{}] Started BLE -> UDP Relay (Dest: {})", index, dest_addr_clone);
        while let Some(data) = notification_stream.next().await {
            if data.uuid == tx_char.uuid {
                match ble_to_udp_socket.send_to(&data.value, &dest_addr_clone).await {
                    Ok(size) => info!("[{}] Relayed {} bytes to UDP", index, size),
                    Err(e) => error!("[{}] Failed to send UDP packet: {}", index, e),
                }
            }
        }
        info!("[{}] BLE notification stream ended", index);
    });

    // Relay UDP -> BLE
    let udp_to_ble_socket = udp_socket.clone();
    let peripheral_clone = peripheral.clone();
    let udp_to_ble_task = tokio::spawn(async move {
        info!("[{}] Started UDP -> BLE Relay (Listen: {})", index, bind_addr);
        let mut buf = vec![0u8; 1024];
        loop {
            match udp_to_ble_socket.recv_from(&mut buf).await {
                Ok((size, addr)) => {
                    info!("[{}] Received {} bytes from UDP ({})", index, size, addr);
                    let payload = &buf[..size];
                    match peripheral_clone
                        .write(&rx_char, payload, btleplug::api::WriteType::WithoutResponse)
                        .await
                    {
                        Ok(_) => info!("[{}] Relayed {} bytes to BLE", index, size),
                        Err(e) => error!("[{}] Failed to write to BLE characteristic: {}", index, e),
                    }
                }
                Err(e) => {
                    error!("[{}] Error receiving UDP packet: {}", index, e);
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = ble_to_udp_task => {
            warn!("[{}] BLE to UDP task exited", index);
        }
        _ = udp_to_ble_task => {
            warn!("[{}] UDP to BLE task exited", index);
        }
    }

    Ok(())
}
