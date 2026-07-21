//! Tips overlay handlers.

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_show_tips(
        &mut self,
        tips: Vec<crate::tips::Tip>,
        starting_index: usize,
        max_seen_id: u32,
        show_mode: crate::models::TipsShowMode,
    ) -> Vec<Command> {
        self.interaction.tips = Some(TipsOverlayState {
            index: starting_index,
            max_seen_id,
            show_mode,
            tips,
        });
        vec![]
    }

    pub(in crate::tui) fn handle_next_tip(&mut self) -> Vec<Command> {
        if let Some(overlay) = &mut self.interaction.tips {
            let len = overlay.tips.len();
            if len > 0 {
                overlay.index = (overlay.index + 1) % len;
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_prev_tip(&mut self) -> Vec<Command> {
        if let Some(overlay) = &mut self.interaction.tips {
            let len = overlay.tips.len();
            if len > 0 {
                overlay.index = (overlay.index + len - 1) % len;
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_set_tips_mode(
        &mut self,
        mode: crate::models::TipsShowMode,
    ) -> Vec<Command> {
        if let Some(overlay) = &mut self.interaction.tips {
            overlay.show_mode = mode;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_close_tips(&mut self) -> Vec<Command> {
        if let Some(overlay) = self.interaction.tips.take() {
            let seen_up_to = overlay
                .current_tip()
                .map(|t| t.id.max(overlay.max_seen_id))
                .unwrap_or(overlay.max_seen_id);
            vec![Command::Tips(
                crate::tui::commands::TipsCommand::SaveState {
                    seen_up_to,
                    show_mode: overlay.show_mode,
                },
            )]
        } else {
            vec![]
        }
    }
}
