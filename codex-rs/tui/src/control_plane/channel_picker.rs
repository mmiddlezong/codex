use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::MultiSelectItem;
use crate::bottom_pane::MultiSelectPicker;
use crate::render::renderable::Renderable;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;

pub(crate) struct ChannelPickerView {
    picker: MultiSelectPicker,
    app_event_tx: AppEventSender,
}

impl ChannelPickerView {
    pub(crate) fn new(
        available_channels: Vec<String>,
        selected_channels: Vec<String>,
        app_event_tx: AppEventSender,
    ) -> Self {
        let items = available_channels
            .into_iter()
            .map(|channel| MultiSelectItem {
                id: channel.clone(),
                name: channel.clone(),
                description: None,
                enabled: selected_channels
                    .iter()
                    .any(|selected| selected == &channel),
            })
            .collect();

        let picker = MultiSelectPicker::builder(
            "Channel subscriptions".to_string(),
            Some(
                "Choose which channels this Codex instance should subscribe to for this run."
                    .to_string(),
            ),
            app_event_tx.clone(),
        )
        .items(items)
        .instructions(vec![
            "Press ".into(),
            Span::raw("space"),
            " to toggle; ".into(),
            Span::raw("c"),
            " to create; ".into(),
            Span::raw("enter"),
            " to confirm; ".into(),
            Span::raw("esc"),
            " to keep current selection".into(),
        ])
        .on_confirm(|ids: &[String], tx: &AppEventSender| {
            tx.send(AppEvent::ControlPlaneChannelsSelected {
                channels: ids.to_vec(),
            });
        })
        .on_cancel(|tx: &AppEventSender| {
            tx.send(AppEvent::ControlPlaneChannelsSelectionCancelled);
        })
        .build();

        Self {
            picker,
            app_event_tx,
        }
    }
}

impl BottomPaneView for ChannelPickerView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            let available_channels = self.picker.all_ids();
            let selected_channels = self.picker.enabled_ids();
            self.picker.close();
            self.app_event_tx
                .send(AppEvent::ControlPlaneCreateChannelRequested {
                    available_channels,
                    selected_channels,
                });
            return;
        }
        self.picker.handle_key_event(key_event);
    }

    fn is_complete(&self) -> bool {
        self.picker.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.picker.close();
        CancellationEvent::Handled
    }
}

impl Renderable for ChannelPickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.picker.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.picker.desired_height(width)
    }
}
