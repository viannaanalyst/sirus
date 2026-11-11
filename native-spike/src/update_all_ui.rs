//! "Atualizar tudo" in the window (ADR-207): starts update_all.rs's threads, keeps their steps in
//! `Shell::updates` for Settings → Avançado → Atualizações, asks before a host install would cut
//! conversations, and hands this Mac's provider results to the provider cards (toast.rs). A child
//! of `shell` (`#[path]`).

use super::*;
use crate::computers::{self, HOME, Tokens};
use crate::update_all::{Job, Step, Target, row_key};

impl Shell {
    /// Hands the page the board.
    pub(super) fn publish_updates(&mut self) {
        self.settings_data
            .insert("updates", self.updates.to_value());
    }

    /// "Atualizar tudo": the app (check, then download and install; the restart stays the
    /// person's), this Mac's CLIs (`mac_providers`, or every one with a one-click update), and
    /// each paired computer's host and CLIs. False when a run is already going.
    pub(super) fn update_all_start(
        &mut self,
        mac_providers: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.updates.running() {
            return false;
        }
        use crate::app_update::Phase;
        if !matches!(
            crate::app_update::status().phase,
            Phase::Checking | Phase::Downloading | Phase::Installed
        ) {
            self.app_update_run(false, true);
        }
        self.update_spawn(Job {
            computer: HOME.to_owned(),
            name: t("Este Mac").to_owned(),
            providers: Some(mac_providers),
            ..Job::default()
        });
        for computer in self.computers.list.clone() {
            self.update_remote(&computer, |job| {
                job.providers = Some(None);
                job.host = Some(false);
            });
        }
        cx.notify();
        true
    }

    /// Where every row stands, read when Atualizações opens; nothing is updated.
    pub(super) fn update_all_probe(&mut self) {
        if self.updates.running() || self.updates.probes > 0 {
            return;
        }
        self.update_spawn(Job {
            computer: HOME.to_owned(),
            probe: true,
            ..Job::default()
        });
        for computer in self.computers.list.clone() {
            self.update_remote(&computer, |job| job.probe = true);
        }
    }

    fn update_remote(&mut self, computer: &computers::Computer, set: impl FnOnce(&mut Job)) {
        let mut job = Job {
            computer: computer.id.clone(),
            name: computer.name.clone(),
            ..Job::default()
        };
        set(&mut job);
        let target = match (computer.address(), computers::Keychain.get(&computer.id)) {
            (Ok(address), Some(token)) => Target::Remote { address, token },
            _ => {
                let failed = Step::Failed(t("Pareie de novo").to_owned());
                for part in ["host", "providers"] {
                    self.updates
                        .rows
                        .insert(row_key(part, &computer.id), failed.clone());
                }
                self.publish_updates();
                return;
            }
        };
        self.count_worker(&job);
        crate::update_all::spawn(job, target, self.native_tx.clone());
        self.publish_updates();
    }

    fn update_spawn(&mut self, job: Job) {
        self.count_worker(&job);
        crate::update_all::spawn(
            job,
            Target::Mac(self.backend.clone()),
            self.native_tx.clone(),
        );
        self.publish_updates();
    }

    fn count_worker(&mut self, job: &Job) {
        if job.probe {
            self.updates.probes += 1;
        } else {
            self.updates.workers += 1;
        }
    }

    /// One message from a thread: a row's step, this Mac's provider results, a question, or the
    /// thread's end.
    pub(super) fn update_all_event(&mut self, value: Value, cx: &mut Context<Self>) {
        let probe = value["probe"].as_bool() == Some(true);
        if value["ended"].is_string() {
            if probe {
                self.updates.probes = self.updates.probes.saturating_sub(1);
            } else {
                self.updates.workers = self.updates.workers.saturating_sub(1);
            }
        } else if probe && self.updates.running() {
            // What a probe read is older than the run that started since.
        } else if let Some(ask) = value.get("ask") {
            let computer = ask["computer"].as_str().unwrap_or("").to_owned();
            let name = ask["name"].as_str().unwrap_or("").to_owned();
            let count = ask["count"].as_u64().unwrap_or(1);
            // Another question on screen wins; the row keeps its own "Instalar agora".
            if self.confirm.is_none() {
                self.update_host_ask(computer, name, count, cx);
            }
        } else if value["results"].is_object() || value["results"].is_array() {
            // This Mac's CLIs: their cards show the result (toast.rs).
            if value["row"].as_str() == Some(row_key("providers", HOME).as_str()) {
                let ok = value["ok"].as_bool().unwrap_or(false);
                for provider in value["providers"].as_array().into_iter().flatten() {
                    if let Some(provider) = provider.as_str() {
                        self.update_answered(provider, ok, &value["results"], cx);
                    }
                }
            }
        } else if let Some(row) = value["row"].as_str() {
            let step: Step = serde_json::from_value(value["step"].clone()).unwrap_or_default();
            self.updates.rows.insert(row.to_owned(), step);
        }
        self.publish_updates();
        cx.notify();
    }

    /// The buttons of the Atualizações rows.
    pub(super) fn update_all_command(&mut self, name: &str, args: &Value, cx: &mut Context<Self>) {
        let text = |key: &str| args[key].as_str().unwrap_or("").to_owned();
        match name {
            "native_update_all" => {
                self.update_all_start(None, cx);
            }
            "native_update_retry" => self.update_retry(&text("row"), cx),
            "native_update_host_now" => {
                let count = args["count"].as_u64().unwrap_or(1);
                self.update_host_ask(text("computer"), text("name"), count, cx);
            }
            _ => {}
        }
    }

    /// "Tentar de novo" (or a row's "Atualizar"): runs that row's part again.
    fn update_retry(&mut self, row: &str, cx: &mut Context<Self>) {
        let Some((part, id)) = row.split_once(':') else {
            return;
        };
        if id == HOME {
            if part == "providers" {
                self.update_spawn(Job {
                    computer: HOME.to_owned(),
                    providers: Some(None),
                    ..Job::default()
                });
            }
        } else if let Some(computer) = self.computers.list.iter().find(|c| c.id == id).cloned() {
            let host = part == "host";
            self.update_remote(&computer, |job| {
                if host {
                    job.host = Some(false);
                } else {
                    job.providers = Some(None);
                }
            });
        }
        cx.notify();
    }

    /// A new host waits for conversations: install now and cut them, or let it install when they
    /// are done (the updater's own rule).
    fn update_host_ask(
        &mut self,
        computer: String,
        name: String,
        count: u64,
        cx: &mut Context<Self>,
    ) {
        let message = if count == 1 {
            tf(
                "Uma conversa está em andamento em {}. Instalar agora corta essa conversa; ela fica salva e pode continuar depois. Ou o Sirus instala sozinho quando ela terminar.",
                &[&name],
            )
        } else {
            tf(
                "{} conversas estão em andamento em {}. Instalar agora corta essas conversas; elas ficam salvas e podem continuar depois. Ou o Sirus instala sozinho quando elas terminarem.",
                &[&count, &name],
            )
        };
        self.ask_choice(
            tf("Instalar o Sirus em {} agora?", &[&name]),
            message,
            t("Atualizar agora"),
            t("Quando terminar"),
            move |this, cx| {
                if let Some(found) = this
                    .computers
                    .list
                    .iter()
                    .find(|c| c.id == computer)
                    .cloned()
                {
                    this.update_remote(&found, |job| job.host = Some(true));
                }
                cx.notify();
            },
            cx,
        );
    }
}
