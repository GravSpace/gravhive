use crate::ipc::IpcClient;
use crate::model::{
    format_duration, format_memory_bytes, ProcessItem, ProcessStatus, TargetSelector,
};
use crate::protocol::{Request, Response};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Row, Table},
    Frame, Terminal,
};
use std::io;
use std::time::{Duration, Instant};

pub async fn run_monit() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = monit_loop(&mut terminal).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

async fn monit_loop<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut selected_idx = 0;
    let mut last_fetch = Instant::now() - Duration::from_secs(10);
    let mut processes: Vec<ProcessItem> = Vec::new();
    let mut recent_logs: Vec<String> = Vec::new();
    let mut status_msg = String::new();

    loop {
        // Poll process list every 1 second
        if last_fetch.elapsed() >= Duration::from_millis(1000) {
            if let Ok(Response::ProcessList { processes: p }) =
                IpcClient::send_request(Request::List).await
            {
                processes = p;
                if selected_idx >= processes.len() && !processes.is_empty() {
                    selected_idx = processes.len() - 1;
                }

                // If a process is selected, fetch recent logs
                if let Some(selected) = processes.get(selected_idx) {
                    if let Ok(Response::ProcessDetails { logs_preview, .. }) =
                        IpcClient::send_request(Request::Describe(TargetSelector::Id(selected.id)))
                            .await
                    {
                        recent_logs = logs_preview;
                    }
                }
            }
            last_fetch = Instant::now();
        }

        let sel = selected_idx;
        let procs = processes.clone();
        let logs = recent_logs.clone();
        let s_msg = status_msg.clone();

        terminal
            .draw(|f| {
                render_ui(f, &procs, sel, &logs, &s_msg);
            })
            .map_err(|e| anyhow::anyhow!("Terminal draw error: {}", e))?;

        // Handle user keyboard events
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Up | KeyCode::Char('k') => {
                        if selected_idx > 0 {
                            selected_idx -= 1;
                            last_fetch = Instant::now() - Duration::from_secs(10);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if !processes.is_empty() && selected_idx < processes.len() - 1 {
                            selected_idx += 1;
                            last_fetch = Instant::now() - Duration::from_secs(10);
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Some(target) = processes.get(selected_idx) {
                            let _ = IpcClient::send_request(Request::Restart(TargetSelector::Id(
                                target.id,
                            )))
                            .await;
                            status_msg = format!("Restart triggered for #{}", target.id);
                            last_fetch = Instant::now() - Duration::from_secs(10);
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(target) = processes.get(selected_idx) {
                            let _ = IpcClient::send_request(Request::Stop(TargetSelector::Id(
                                target.id,
                            )))
                            .await;
                            status_msg = format!("Stop triggered for #{}", target.id);
                            last_fetch = Instant::now() - Duration::from_secs(10);
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Some(target) = processes.get(selected_idx) {
                            let _ = IpcClient::send_request(Request::Delete(TargetSelector::Id(
                                target.id,
                            )))
                            .await;
                            status_msg = format!("Delete triggered for #{}", target.id);
                            last_fetch = Instant::now() - Duration::from_secs(10);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn render_ui(
    f: &mut Frame,
    processes: &[ProcessItem],
    selected_idx: usize,
    logs: &[String],
    status_msg: &str,
) {
    let size = f.area();

    // Main layout: Header, Body, Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Content
            Constraint::Length(3), // Footer
        ])
        .split(size);

    // 1. Header
    let active_count = processes
        .iter()
        .filter(|p| p.status == ProcessStatus::Online)
        .count();
    let total_mem: u64 = processes.iter().map(|p| p.memory_bytes).sum();

    let header_text = vec![Line::from(vec![
        Span::styled(
            " GRAVHIVE MONITOR ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | Active: "),
        Span::styled(
            format!("{}/{}", active_count, processes.len()),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | Total Memory: "),
        Span::styled(
            format_memory_bytes(total_mem),
            Style::default().fg(Color::Yellow),
        ),
    ])];
    let header = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Gravhive Real-time Dashboard "),
    );
    f.render_widget(header, chunks[0]);

    // 2. Body split: Left (Processes), Right (Detail & Logs)
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[1]);

    // Left: Process List Table
    let rows: Vec<Row> = processes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_selected = i == selected_idx;
            let status_color = match p.status {
                ProcessStatus::Online => Color::Green,
                ProcessStatus::Stopping | ProcessStatus::Restarting => Color::Yellow,
                ProcessStatus::Stopped => Color::DarkGray,
                ProcessStatus::Errored => Color::Red,
            };

            let row_style = if is_selected {
                Style::default().bg(Color::Rgb(40, 50, 70)).fg(Color::White)
            } else {
                Style::default()
            };

            let prefix = if is_selected { "► " } else { "  " };

            let _ = status_color;

            Row::new(vec![
                format!("{}{}", prefix, p.id),
                p.name.clone(),
                format!("{:?}", p.status),
                format!("{:.1}%", p.cpu_percent),
                format_memory_bytes(p.memory_bytes),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["ID", "NAME", "STATUS", "CPU", "MEM"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().borders(Borders::ALL).title(" Processes "));

    f.render_widget(table, body_chunks[0]);

    // Right: Split into Detail (top) and Logs (bottom)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(8)])
        .split(body_chunks[1]);

    // Detail Panel
    if let Some(sel_proc) = processes.get(selected_idx) {
        let cpu_gauge = Gauge::default()
            .block(Block::default().title(" CPU Usage "))
            .gauge_style(Style::default().fg(Color::Magenta))
            .ratio((sel_proc.cpu_percent.clamp(0.0, 100.0) / 100.0) as f64)
            .label(format!("{:.1}%", sel_proc.cpu_percent));

        let detail_text = vec![
            Line::from(vec![
                Span::raw("App: "),
                Span::styled(&sel_proc.name, Style::default().fg(Color::Cyan)),
                Span::raw(" | PID: "),
                Span::styled(sel_proc.pid.to_string(), Style::default().fg(Color::Yellow)),
                Span::raw(" | Mode: "),
                Span::raw(format!("{:?}", sel_proc.exec_mode)),
            ]),
            Line::from(vec![
                Span::raw("Uptime: "),
                Span::raw(format_duration(sel_proc.uptime_secs)),
                Span::raw(" | Restarts: "),
                Span::styled(
                    sel_proc.restarts.to_string(),
                    Style::default().fg(Color::Red),
                ),
                Span::raw(" | Mem: "),
                Span::styled(
                    format_memory_bytes(sel_proc.memory_bytes),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        ];

        let detail_p = Paragraph::new(detail_text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Process Metadata "),
        );

        let detail_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Length(3)])
            .split(right_chunks[0]);

        f.render_widget(detail_p, detail_split[0]);
        f.render_widget(cpu_gauge, detail_split[1]);
    } else {
        let empty = Paragraph::new("No processes active").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Process Metadata "),
        );
        f.render_widget(empty, right_chunks[0]);
    }

    // Logs Panel
    let log_items: Vec<ListItem> = logs
        .iter()
        .map(|l| ListItem::new(Line::from(l.as_str())))
        .collect();
    let logs_widget = List::new(log_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Live Logs (STDOUT / STDERR) "),
    );
    f.render_widget(logs_widget, right_chunks[1]);

    // 3. Footer: Shortcuts
    let footer_text = if !status_msg.is_empty() {
        format!(
            "INFO: {} | [↑/↓] Select | [R] Restart | [S] Stop | [D] Delete | [Q] Quit",
            status_msg
        )
    } else {
        "[↑/↓] Select Process | [R] Restart | [S] Stop | [D] Delete | [Q] Quit".to_string()
    };
    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
