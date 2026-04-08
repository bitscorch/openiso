// Tindeq Progressor BLE API: https://tindeq.com/progressor_api/

use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter, WriteType};
use btleplug::platform::Manager;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph},
};
use std::fs;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};
use uuid::Uuid;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x7e4e1701_1ea6_40c9_9dcc_13d34ffead57);
const DATA_CHAR_UUID: Uuid = Uuid::from_u128(0x7e4e1702_1ea6_40c9_9dcc_13d34ffead57);
const CTRL_CHAR_UUID: Uuid = Uuid::from_u128(0x7e4e1703_1ea6_40c9_9dcc_13d34ffead57);

const CMD_START_WEIGHT_MEAS: u8 = 0x65;
const CMD_STOP_WEIGHT_MEAS: u8 = 0x66;
const CMD_TARE_SCALE: u8 = 0x64;

const RES_WEIGHT_MEAS: u8 = 0x01;

struct AppState {
    force_history: Vec<(f64, f64)>,
    max_weight: f32,
    current_weight: f32,
    start: Instant,
}

fn ui(frame: &mut ratatui::Frame, state: &AppState) {
    let elapsed = state.start.elapsed().as_secs_f64();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(10)])
        .split(frame.area());

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled("  Force: ", Style::default().fg(Color::White)),
        Span::styled(
            format!("{:5.1} kg", state.current_weight),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   "),
        Span::styled("MVC: ", Style::default().fg(Color::White)),
        Span::styled(
            format!("{:5.1} kg", state.max_weight),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("   "),
        Span::styled("Time: ", Style::default().fg(Color::White)),
        Span::styled(
            format!("{:.1}s", elapsed),
            Style::default().fg(Color::Green),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" OpenISO "));
    frame.render_widget(header, chunks[0]);

    // Chart
    let x_min = (elapsed - 30.0).max(0.0);
    let y_max = if state.max_weight > 0.0 {
        (state.max_weight * 1.2) as f64
    } else {
        10.0
    };

    let x_max = elapsed.max(1.0);
    let x_range = x_max - x_min;
    let x_step = (x_range / 6.0).max(1.0);
    let x_labels: Vec<String> = (0..=6)
        .map(|i| format!("{:.0}", x_min + i as f64 * x_step))
        .collect();

    let y_step = y_max / 5.0;
    let y_labels: Vec<String> = (0..=5)
        .map(|i| format!("{:.0}", i as f64 * y_step))
        .collect();

    // Horizontal grid lines at each Y tick
    let grid_lines: Vec<Vec<(f64, f64)>> = (1..=5)
        .map(|i| {
            let y = i as f64 * y_step;
            vec![(x_min, y), (x_max, y)]
        })
        .collect();
    let grid_datasets: Vec<Dataset> = grid_lines
        .iter()
        .map(|line| {
            Dataset::default()
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::DarkGray))
                .data(line)
        })
        .collect();

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&state.force_history);

    let mut datasets = grid_datasets;
    datasets.push(dataset);

    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL))
        .x_axis(
            Axis::default()
                .title("Time (s)")
                .bounds([x_min, x_max])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .title("Force (kg)")
                .bounds([0.0, y_max])
                .labels(y_labels),
        );
    frame.render_widget(chart, chunks[1]);
}

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

    // Set up session CSV
    fs::create_dir_all("sessions")?;
    let session_path = format!("sessions/{}.csv", Local::now().format("%Y-%m-%d_%H-%M-%S"));
    let mut csv = BufWriter::new(fs::File::create(&session_path)?);
    writeln!(csv, "elapsed_s,weight_kg")?;

    // Start weight measurement
    progressor
        .write(ctrl_char, &[CMD_START_WEIGHT_MEAS], WriteType::WithResponse)
        .await?;

    // Enter TUI
    let mut terminal = ratatui::init();
    let mut state = AppState {
        force_history: Vec::new(),
        max_weight: 0.0,
        current_weight: 0.0,
        start: Instant::now(),
    };

    let mut stream = progressor.notifications().await?;
    let mut tick = tokio::time::interval(Duration::from_millis(50));

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                Some(notification) = stream.next() => {
                    if notification.uuid == DATA_CHAR_UUID && !notification.value.is_empty() {
                        let data = &notification.value;
                        if data[0] == RES_WEIGHT_MEAS && data.len() >= 10 {
                            for chunk in data[2..].chunks(8) {
                                if chunk.len() == 8 {
                                    let weight = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
                                    if weight > state.max_weight {
                                        state.max_weight = weight;
                                    }
                                    state.current_weight = weight;
                                    let elapsed = state.start.elapsed().as_secs_f64();
                                    state.force_history.push((elapsed, weight as f64));
                                    writeln!(csv, "{},{}", elapsed, weight).ok();
                                }
                            }
                        }
                    }
                }
                _ = tick.tick() => {
                    while event::poll(Duration::ZERO)? {
                        if let Event::Key(key) = event::read()? {
                            if key.code == KeyCode::Char('q')
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                            {
                                return Ok(());
                            }
                        }
                    }
                    terminal.draw(|frame| ui(frame, &state))?;
                }
            }
        }
    }
    .await;

    // Restore terminal
    ratatui::restore();

    csv.flush()?;
    progressor
        .write(ctrl_char, &[CMD_STOP_WEIGHT_MEAS], WriteType::WithResponse)
        .await?;
    progressor.disconnect().await?;

    println!("MVC: {:5.1} kg", state.max_weight);
    println!("Session saved to {}", session_path);
    println!("Disconnected.");

    result
}
