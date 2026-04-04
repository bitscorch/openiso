use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter, WriteType};
use btleplug::platform::Manager;
use futures::StreamExt;
use std::time::Duration;
use uuid::Uuid;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x7e4e1701_1ea6_40c9_9dcc_13d34ffead57);
const DATA_CHAR_UUID: Uuid = Uuid::from_u128(0x7e4e1702_1ea6_40c9_9dcc_13d34ffead57);
const CTRL_CHAR_UUID: Uuid = Uuid::from_u128(0x7e4e1703_1ea6_40c9_9dcc_13d34ffead57);

const CMD_START_WEIGHT_MEAS: u8 = 101;
const CMD_STOP_WEIGHT_MEAS: u8 = 102;
const CMD_TARE_SCALE: u8 = 100;

const RES_WEIGHT_MEAS: u8 = 1;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Scanning for Tindeq Progressor...");

    let adapter = Manager::new()
        .await?
        .adapters()
        .await?
        .into_iter()
        .next()
        .expect("No BLE adapter found");

    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let peripherals = adapter.peripherals().await?;
    let progressor = peripherals
        .into_iter()
        .filter_map(|p| {
            let name = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(p.properties())
                    .ok()
                    .flatten()
                    .and_then(|props| props.local_name)
            });
            if name.as_ref().is_some_and(|n| n.starts_with("Progressor")) {
                Some(p)
            } else {
                None
            }
        })
        .next()
        .expect("No Progressor found");

    println!("Found Progressor, connecting...");
    progressor.connect().await?;
    progressor.discover_services().await?;

    let chars = progressor.characteristics();
    let data_char = chars
        .iter()
        .find(|c| c.uuid == DATA_CHAR_UUID)
        .expect("Data characteristic not found");
    let ctrl_char = chars
        .iter()
        .find(|c| c.uuid == CTRL_CHAR_UUID)
        .expect("Control characteristic not found");

    // Subscribe to notifications
    progressor.subscribe(data_char).await?;

    // Tare the scale
    progressor
        .write(ctrl_char, &[CMD_TARE_SCALE], WriteType::WithResponse)
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Start weight measurement
    println!("Starting measurement... (Ctrl+C to stop)");
    progressor
        .write(ctrl_char, &[CMD_START_WEIGHT_MEAS], WriteType::WithResponse)
        .await?;

    let mut stream = progressor.notifications().await?;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            Some(notification) = stream.next() => {
                if notification.uuid == DATA_CHAR_UUID && !notification.value.is_empty() {
                    let data = &notification.value;
                    if data[0] == RES_WEIGHT_MEAS && data.len() >= 10 {
                        for chunk in data[2..].chunks(8) {
                            if chunk.len() == 8 {
                                let weight = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
                                let timestamp = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
                                println!("{:.1} kg  (t: {} µs)", weight, timestamp);
                            }
                        }
                    }
                }
            }
            _ = &mut ctrl_c => {
                println!("\nStopping measurement...");
                break;
            }
        }
    }

    progressor
        .write(ctrl_char, &[CMD_STOP_WEIGHT_MEAS], WriteType::WithResponse)
        .await?;
    progressor.disconnect().await?;
    println!("Disconnected.");

    Ok(())
}
