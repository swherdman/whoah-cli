pub mod components;
pub mod layout;
pub mod theme;

use std::io::{self, Stdout};
use std::time::Duration;

use color_eyre::Result;
use futures::StreamExt;
use crossterm::event::EventStream;
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture,
};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::event::Event;

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    event_rx: mpsc::UnboundedReceiver<Event>,
    event_tx: mpsc::UnboundedSender<Event>,
    task: Option<tokio::task::JoinHandle<()>>,
    cancel: CancellationToken,
}

impl Tui {
    pub fn new() -> Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Ok(Self {
            terminal,
            event_rx,
            event_tx,
            task: None,
            cancel: CancellationToken::new(),
        })
    }

    /// Get a sender for pushing app events into the TUI event loop.
    pub fn event_tx(&self) -> mpsc::UnboundedSender<Event> {
        self.event_tx.clone()
    }

    pub fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        io::stdout().execute(EnableMouseCapture)?;
        self.terminal.hide_cursor()?;
        self.terminal.clear()?;

        let cancel = self.cancel.clone();
        let event_tx = self.event_tx.clone();
        let tick_rate = Duration::from_millis(250);
        let render_rate = Duration::from_millis(33); // ~30fps

        self.task = Some(tokio::spawn(async move {
            let mut event_stream = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);
            let mut render_interval = tokio::time::interval(render_rate);

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick_interval.tick() => {
                        let _ = event_tx.send(Event::Tick);
                    }
                    _ = render_interval.tick() => {
                        let _ = event_tx.send(Event::Render);
                    }
                    Some(Ok(event)) = event_stream.next() => {
                        let _ = event_tx.send(Event::Terminal(event));
                    }
                }
            }
        }));

        Ok(())
    }

    pub fn exit(&mut self) -> Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            // Don't block forever — the task will stop on cancel
            tokio::task::block_in_place(|| {
                let _ = tokio::runtime::Handle::current().block_on(async {
                    let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
                });
            });
        }
        io::stdout().execute(DisableMouseCapture)?;
        io::stdout().execute(LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }

    pub fn draw(&mut self, f: impl FnOnce(&mut Frame)) -> Result<()> {
        self.terminal.draw(f)?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Safety net — try to restore terminal even on panic
        self.cancel.cancel();
        let _ = io::stdout().execute(DisableMouseCapture);
        let _ = io::stdout().execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
    }
}
