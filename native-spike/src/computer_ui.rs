//! Computers in the window (ADR-133): the rows under the project switcher's list and what
//! Settings → Conexões does for them: pair a computer with its six-digit code, use it, remove it.
//! The pairing runs off the interface thread; its token goes straight to the Keychain and is
//! never shown, logged or put in a file. A child of `shell` (`#[path]`).

use super::*;
use crate::computers::{self, HOME, Status, Tokens};
use crate::remote_tls::Address;

/// The text of the "add a computer" fields on Settings → Conexões.
#[derive(Default)]
pub(crate) struct AddForm {
    pub address: String,
    pub code: String,
    pub busy: bool,
}

pub(crate) const ADDRESS: &str = "computer-address";
pub(crate) const CODE: &str = "computer-code";

/// Where a connection is, in words.
pub(crate) fn status_text(status: Status) -> &'static str {
    match status {
        Status::Open => t("Conectado"),
        Status::Connecting => t("Conectando…"),
        Status::Lost => t("Sem conexão, tentando de novo"),
        Status::Unauthorized => t("Pareie de novo"),
    }
}

/// A finished edit of one of the form's fields. True when the key was one of its own.
pub(crate) fn is_field(key: &str) -> bool {
    key == ADDRESS || key == CODE
}

impl Shell {
    /// Hands the Conexões page what it draws: the computers, which is in use, the form and the note.
    pub(super) fn publish_computers(&mut self) {
        let list: Vec<Value> = self
            .computers
            .list
            .iter()
            .map(|c| json!({ "id": c.id, "name": c.name, "address": c.address }))
            .collect();
        let value = json!({
            "list": list,
            "active": self.computers.active,
            "status": status_text(self.computers.status),
            "address": self.computers.add.address,
            "code": self.computers.add.code,
            "busy": self.computers.add.busy,
            "note": self.computers.note,
        });
        self.settings_data.insert("computers", value);
    }

    /// The buttons and fields of the Computadores group on Settings → Conexões.
    pub(super) fn computer_command(&mut self, name: &str, args: &Value, cx: &mut Context<Self>) {
        let text = |key: &str| args[key].as_str().unwrap_or("").to_owned();
        match name {
            "native_computer_field" => {
                let value = text("text").trim().to_owned();
                match text("key").as_str() {
                    ADDRESS => self.computers.add.address = value,
                    CODE => self.computers.add.code = value,
                    _ => {}
                }
            }
            "native_computer_use" => self.switch_computer(&text("id"), cx),
            "native_computer_remove" => {
                let id = text("id");
                if self.computers.active == id {
                    self.switch_computer(HOME, cx);
                }
                match computers::forget(&mut self.computers.list, &id, &computers::Keychain) {
                    Ok(()) => self.computers.note = None,
                    Err(error) => self.computers.note = Some(error),
                }
            }
            // A code still being typed counts: the click on Parear leaves the field.
            "native_computer_pair" => {
                if let Some((key, text)) = self.setting_edit.take() {
                    match key {
                        ADDRESS => self.computers.add.address = text.trim().to_owned(),
                        CODE => self.computers.add.code = text.trim().to_owned(),
                        _ => self.setting_edit = Some((key, text)),
                    }
                }
                self.pair_computer()
            }
            _ => {}
        }
        self.publish_computers();
        cx.notify();
    }

    fn pair_computer(&mut self) {
        if self.computers.add.busy {
            return;
        }
        let address = match Address::parse(&self.computers.add.address) {
            Ok(address) => address,
            Err(error) => {
                self.computers.note = Some(error);
                return;
            }
        };
        let code = self.computers.add.code.clone();
        if computers::normalize_code(&code).is_none() {
            self.computers.note = Some(t("O código tem seis dígitos.").to_owned());
            return;
        }
        self.computers.add.busy = true;
        self.computers.note = Some(t("Pareando…").to_owned());
        let mut list = self.computers.list.clone();
        self.native_task("computer_pair", move || {
            match computers::pair(&address, &code, &mut list, &computers::Keychain) {
                Ok(computer) => json!({ "ok": true, "id": computer.id }),
                Err(error) => json!({ "ok": false, "message": error }),
            }
        });
    }

    /// The pairing finished: the list is read again from where it was saved.
    pub(super) fn computer_paired(&mut self, result: &Value, cx: &mut Context<Self>) {
        self.computers.add.busy = false;
        if result["ok"].as_bool() == Some(true) {
            self.computers.list = computers::load();
            self.computers.add = AddForm::default();
            self.computers.note = Some(t("Computador pareado.").to_owned());
            // Pairing again the computer in use replaces a token the host no longer accepts.
            let id = result["id"].as_str().unwrap_or("").to_owned();
            if self.computers.active == id {
                let found = self
                    .computers
                    .list
                    .iter()
                    .find(|c| c.id == id)
                    .and_then(|c| c.address().ok());
                if let (Some(address), Some(token)) = (found, computers::Keychain.get(&id)) {
                    self.computers.status = Status::Connecting;
                    self.backend.connect_remote(&id, address, token);
                }
            }
        } else {
            self.computers.note = Some(result["message"].as_str().unwrap_or("").to_owned());
        }
        self.publish_computers();
        cx.notify();
    }

    /// Height the computers add under the project list.
    pub(super) fn computers_menu_height(&self) -> f32 {
        if self.computers.list.is_empty() {
            0.
        } else {
            8. + 32. * (self.computers.list.len() + 1) as f32
        }
    }

    /// This Mac and the paired computers, as rows under the project list; the one in use is
    /// checked, and choosing another makes it the computer of the whole window.
    pub(super) fn computers_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.computers.list.is_empty() {
            return None;
        }
        let mut entries = vec![(HOME.to_owned(), t("Este Mac").to_owned())];
        entries.extend(
            self.computers
                .list
                .iter()
                .map(|c| (c.id.clone(), c.name.clone())),
        );
        let rows = entries.into_iter().enumerate().map(|(at, (id, name))| {
            let active = self.computers.active == id;
            let status = (active && id != HOME).then(|| status_text(self.computers.status));
            div()
                .id(("computer-pick", at))
                .h(px(32.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(content(0.85))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_click(cx.listener(move |this, _, _, cx| this.switch_computer(&id, cx)))
                .child(div().w(px(14.)).flex_shrink_0().when(active, |d| {
                    d.child(icon("check", 13.).text_color(content(0.8)))
                }))
                .child(div().min_w_0().truncate().child(name))
                .child(div().flex_1())
                .children(status.map(|s| {
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.45))
                        .child(s)
                }))
                .into_any_element()
        });
        Some(
            div()
                .border_t_1()
                .border_color(content(0.08))
                .p(px(4.))
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element(),
        )
    }
}

// Moving a conversation to another computer (computer_move.rs).
impl Shell {
    /// "Mover para …" rows for a conversation's menu, one per other computer. Dimmed while the
    /// conversation works: the host refuses to move a running one.
    pub(super) fn move_menu_items(&self, index: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.computers.list.is_empty() || self.computers.moving {
            return Vec::new();
        }
        let conversation = &self.conversations[index];
        let ready = !conversation.working() && !conversation.session_id.is_empty();
        let mut targets = Vec::new();
        if self.computers.active != HOME {
            targets.push((HOME.to_owned(), t("Este Mac").to_owned()));
        }
        targets.extend(
            self.computers
                .list
                .iter()
                .filter(|c| c.id != self.computers.active)
                .map(|c| (c.id.clone(), c.name.clone())),
        );
        targets
            .into_iter()
            .enumerate()
            .map(|(at, (id, name))| {
                let row = div()
                    .id(("menu-move", at))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .child(icon("upload", 14.).text_color(content(0.65)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(tf("Mover para {}", &[&name])),
                    );
                if ready {
                    row.cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.ask_move(index, id.clone(), cx);
                        }))
                        .into_any_element()
                } else {
                    row.opacity(0.4).into_any_element()
                }
            })
            .collect()
    }

    fn ask_move(&mut self, index: usize, target: String, cx: &mut Context<Self>) {
        let name = self.computers.name_of(&target);
        let title = tf(
            "Mover \"{}\" para {}?",
            &[&self.conversations[index].title, &name],
        );
        let message = tf(
            "O trabalho da conversa, inclusive o que ainda não foi commitado, vai para uma branch sirus/move-… no GitHub do projeto. {} baixa essa branch e continua a conversa lá, com o texto até aqui. Esta conversa fica marcada como movida.",
            &[&name],
        );
        self.ask(
            title,
            message,
            t("Mover"),
            move |this, cx| this.start_move(index, target, cx),
            cx,
        );
    }

    fn start_move(&mut self, index: usize, target: String, cx: &mut Context<Self>) {
        let Some(project) = self.project_of(index) else {
            return;
        };
        let plan = crate::computer_move::Plan {
            session_id: self.conversations[index].session_id.clone(),
            project_id: self.projects[project].id.clone(),
            source_label: self.computers.active_name(),
            target_label: self.computers.name_of(&target),
        };
        // The target: this Mac through the window's own link, or another computer through a
        // connection of its own (its token is read from the Keychain here, never shown).
        let backend = self.backend.clone();
        let peer: Result<
            Box<dyn Fn() -> Result<Box<dyn crate::computer_move::Peer>, String> + Send>,
            String,
        > = if target == HOME {
            let backend = backend.clone();
            Ok(Box::new(move || {
                Ok(Box::new(crate::computer_move::Linked {
                    backend: backend.clone(),
                    local: true,
                }) as Box<dyn crate::computer_move::Peer>)
            }))
        } else {
            let found = self
                .computers
                .list
                .iter()
                .find(|c| c.id == target)
                .and_then(|c| c.address().ok());
            match (found, computers::Keychain.get(&target)) {
                (Some(address), Some(token)) => Ok(Box::new(move || {
                    crate::computer_move::Oneshot::open(&address, &token)
                        .map(|o| Box::new(o) as Box<dyn crate::computer_move::Peer>)
                })),
                _ => Err(t("Esse computador perdeu o pareamento: pareie de novo.").to_owned()),
            }
        };
        let open = match peer {
            Ok(open) => open,
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        };
        self.computers.moving = true;
        self.problem = Some(t("Movendo a conversa…").to_owned());
        let results = self.native_tx.clone();
        std::thread::spawn(move || {
            let source = crate::computer_move::Linked {
                backend,
                local: false,
            };
            let outcome = open().and_then(|target_peer| {
                crate::computer_move::run(&source, target_peer.as_ref(), &plan)
            });
            let value = match outcome {
                Ok(moved) => {
                    json!({ "ok": true, "target": target, "sessionId": moved.session_id, "projectId": moved.project_id })
                }
                Err(message) => json!({ "ok": false, "message": message }),
            };
            let _ = results.send_blocking(("computer_move", value));
        });
        cx.notify();
    }

    /// A move finished: on success the window goes to the target computer with the new
    /// conversation open there.
    pub(super) fn computer_moved(&mut self, result: &Value, cx: &mut Context<Self>) {
        self.computers.moving = false;
        self.problem = None;
        if result["ok"].as_bool() == Some(true) {
            let target = result["target"].as_str().unwrap_or("").to_owned();
            let session = result["sessionId"].as_str().unwrap_or("").to_owned();
            let project = result["projectId"].as_str().unwrap_or("").to_owned();
            self.open_on_arrival(&target, session, project);
            self.switch_computer(&target, cx);
        } else {
            self.problem = Some(result["message"].as_str().unwrap_or("").to_owned());
        }
        self.publish_computers();
        cx.notify();
    }
}
