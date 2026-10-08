use crate::{
    discovery::{Topic, display_type},
    schema::{Registry, embedded_registry},
    transport::{Connection, MAX_PAYLOAD, Sample},
    views,
};
use egui::{Color32, RichText};
use hiroz::dynamic::DynamicMessage;
use std::{sync::Arc, time::Duration};
use web_time::Instant;

pub struct Inspector {
    endpoint: String,
    domain: Option<usize>,
    filter: String,
    connection: Option<Connection>,
    registry: Arc<Registry>,
    selected: Option<Topic>,
    paused: bool,
    shown: Option<Sample>,
    decoded: Option<Result<DynamicMessage, String>>,
    last_decode: Instant,
    last_meter: Instant,
    previous_counts: (u64, u64),
    rate: (f64, f64),
    tab: usize,
}

impl Inspector {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_custom_types(cc, Registry::new())
    }

    pub fn new_with_custom_types(cc: &eframe::CreationContext<'_>, custom_types: Registry) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let (endpoint, domain) = (None, None);
        #[cfg(target_arch = "wasm32")]
        let (endpoint, domain) = web_sys::window()
            .and_then(|w| w.location().href().ok())
            .and_then(|href| web_sys::Url::new(&href).ok())
            .map(|url| {
                (
                    url.search_params().get("endpoint"),
                    url.search_params()
                        .get("domain")
                        .and_then(|v| v.parse().ok()),
                )
            })
            .unwrap_or_default();
        Self::with_custom_types(cc, endpoint, domain, custom_types)
    }

    pub fn with_options(
        cc: &eframe::CreationContext<'_>,
        endpoint: Option<String>,
        domain: Option<usize>,
    ) -> Self {
        Self::with_custom_types(cc, endpoint, domain, Registry::new())
    }

    pub fn with_custom_types(
        cc: &eframe::CreationContext<'_>,
        endpoint: Option<String>,
        domain: Option<usize>,
        custom_types: Registry,
    ) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut_of(egui::Theme::Dark, |s| {
            s.spacing.item_spacing = egui::vec2(10.0, 9.0);
            s.spacing.button_padding = egui::vec2(12.0, 8.0);
            s.visuals.panel_fill = Color32::from_rgb(10, 19, 38);
            s.visuals.window_fill = Color32::from_rgb(17, 32, 55);
            s.visuals.extreme_bg_color = Color32::from_rgb(7, 14, 29);
            s.visuals.faint_bg_color = Color32::from_rgb(22, 39, 64);
            s.visuals.override_text_color = Some(Color32::from_rgb(221, 233, 247));
            s.visuals.selection.bg_fill = Color32::from_rgb(24, 88, 112);
            s.visuals.selection.stroke = egui::Stroke::new(1.0, CYAN);
            s.visuals.widgets.inactive.bg_fill = Color32::from_rgb(25, 43, 70);
            s.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(20, 35, 59);
            s.visuals.widgets.hovered.bg_fill = Color32::from_rgb(34, 70, 99);
            s.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(29, 58, 86);
            s.visuals.widgets.active.bg_fill = Color32::from_rgb(24, 102, 124);
            s.visuals.hyperlink_color = CYAN;
        });
        let connection = endpoint
            .as_ref()
            .map(|e| Connection::open(e.trim().to_owned()));
        let endpoint = endpoint.unwrap_or_else(|| {
            if cfg!(target_arch = "wasm32") {
                "ws/127.0.0.1:7448".into()
            } else {
                "tcp/127.0.0.1:7447".into()
            }
        });
        let mut registry = embedded_registry();
        registry.extend(custom_types);
        Self {
            endpoint,
            domain,
            filter: String::new(),
            connection,
            registry: Arc::new(registry),
            selected: None,
            paused: false,
            shown: None,
            decoded: None,
            last_decode: Instant::now(),
            last_meter: Instant::now(),
            previous_counts: (0, 0),
            rate: (0.0, 0.0),
            tab: 0,
        }
    }
    fn reset_sample(&mut self) {
        self.shown = None;
        self.decoded = None;
        self.paused = false;
        self.previous_counts = (0, 0);
        self.rate = (0.0, 0.0);
        self.last_meter = Instant::now();
    }
}

impl eframe::App for Inspector {
    fn ui(&mut self, root_ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(100));
        let (
            status,
            connected,
            topics,
            domains,
            latest,
            counts,
            active_key,
            error,
            subscription_ready,
            reflected_schema,
            reflection_pending,
            reflection_error,
        ) = if let Some(connection) = &self.connection {
            let s = connection.snapshot.lock().unwrap();
            (
                s.status.clone(),
                s.connected,
                s.topics.clone(),
                s.domains.clone(),
                s.latest.clone(),
                (s.samples, s.bytes),
                s.selected.clone(),
                s.subscription_error.clone(),
                s.subscription_ready,
                s.reflected_schema.clone(),
                s.reflection_pending,
                s.reflection_error.clone(),
            )
        } else {
            (
                "Ready to connect".into(),
                false,
                vec![],
                vec![],
                None,
                (0, 0),
                None,
                None,
                false,
                None,
                false,
                None,
            )
        };
        let elapsed = self.last_meter.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            self.rate = (
                counts.0.saturating_sub(self.previous_counts.0) as f64 / elapsed,
                counts.1.saturating_sub(self.previous_counts.1) as f64 / elapsed,
            );
            self.previous_counts = counts;
            self.last_meter = Instant::now();
        }
        if !self.paused
            && self.last_decode.elapsed() >= Duration::from_millis(100)
            && self.selected.as_ref().map(|t| &t.key) == active_key.as_ref()
            && let Some(sample) = latest
            && self
                .shown
                .as_ref()
                .is_none_or(|s| s.sequence != sample.sequence)
        {
            if let Some(topic) = &self.selected {
                self.decoded = Some(if sample.size > MAX_PAYLOAD {
                    Err(format!(
                        "Payload exceeds the {} MiB inspection limit; counted but not retained.",
                        MAX_PAYLOAD / 1024 / 1024
                    ))
                } else {
                    match &reflected_schema {
                        Some(schema) => Registry::decode_with_schema(schema, &sample.bytes),
                        None => self
                            .registry
                            .decode(&topic.type_name, &topic.hash, &sample.bytes),
                    }
                });
            }
            self.shown = Some(sample);
            self.last_decode = Instant::now();
        }
        egui::Panel::top("connection").show(root_ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                mark(ui, 30.0);
                ui.label(RichText::new("INSPECTOR").strong().size(23.0));
                ui.label(RichText::new("/  ZENOH").size(15.0).color(CYAN));
                ui.separator();
                badge(
                    ui,
                    if connected { "LIVE" } else { "OFFLINE" },
                    if connected { MINT } else { AMBER },
                );
            });
            ui.add_space(5.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("ROUTER").small().color(MUTED));
                ui.add_enabled(
                    self.connection.is_none(),
                    egui::TextEdit::singleline(&mut self.endpoint)
                        .desired_width(360.0)
                        .hint_text("tcp/rover:7447 or ws/rover:7448"),
                );
                if self.connection.is_none() {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Connect & discover").strong())
                                .fill(Color32::from_rgb(20, 107, 122)),
                        )
                        .clicked()
                    {
                        self.reset_sample();
                        self.selected = None;
                        self.connection = Some(Connection::open(self.endpoint.trim().to_owned()));
                    }
                } else if ui.button("Disconnect").clicked() {
                    self.connection = None;
                    self.selected = None;
                    self.reset_sample();
                }
                ui.label(RichText::new(&status).small().color(if connected {
                    MINT
                } else {
                    AMBER
                }));
            });
            ui.add_space(8.0);
        });
        egui::Panel::bottom("footer").show(root_ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} DOMAINS", domains.len()))
                        .small()
                        .color(CYAN),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("{} TOPICS", topics.len()))
                        .small()
                        .color(MINT),
                );
                ui.separator();
                ui.weak(format!("{} embedded schemas", self.registry.len()));
                ui.separator();
                ui.weak("Live graph · latest sample only");
            });
        });
        egui::Panel::left("topics")
            .default_size(350.0)
            .min_size(260.0)
            .resizable(true)
            .show(root_ui, |ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("DISCOVERED DOMAINS")
                        .small()
                        .strong()
                        .color(CYAN),
                );
                let previous_domain = self.domain;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.domain, None, "All domains");
                    for domain in &domains {
                        ui.selectable_value(
                            &mut self.domain,
                            Some(*domain),
                            RichText::new(format!("{domain}")).color(domain_color(*domain)),
                        )
                        .on_hover_text(format!(
                            "{} topics",
                            topics.iter().filter(|t| t.domain == *domain).count()
                        ));
                    }
                    if let Some(domain) = self.domain
                        && !domains.contains(&domain)
                    {
                        ui.label(RichText::new(format!("{domain} · not advertised")).color(AMBER));
                    }
                });
                if self.domain != previous_domain {
                    self.selected = None;
                    self.reset_sample();
                    if let Some(c) = &self.connection {
                        c.select(None);
                    }
                }
                ui.add_space(12.0);
                ui.heading("Topics");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Search topic or message type…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(5.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let filter = self.filter.to_lowercase();
                    let visible: Vec<_> = topics
                        .iter()
                        .filter(|t| {
                            self.domain.is_none_or(|d| t.domain == d)
                                && format!("{} {}", t.name, t.type_name)
                                    .to_lowercase()
                                    .contains(&filter)
                        })
                        .collect();
                    for topic in &visible {
                        let selected = self.selected.as_ref().is_some_and(|t| t.key == topic.key);
                        let embedded_schema =
                            self.registry.get(&topic.type_name, &topic.hash).is_ok();
                        let runtime_schema = selected && reflected_schema.is_some();
                        let schema_ok = embedded_schema || runtime_schema;
                        let color = domain_color(topic.domain);
                        let response = egui::Frame::new()
                            .fill(if selected {
                                Color32::from_rgb(21, 57, 83)
                            } else {
                                Color32::from_rgb(17, 31, 53)
                            })
                            .stroke(egui::Stroke::new(
                                1.0,
                                if selected {
                                    color
                                } else {
                                    Color32::from_rgb(30, 49, 77)
                                },
                            ))
                            .corner_radius(8)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(&topic.name).strong().size(15.0));
                                ui.label(
                                    RichText::new(display_type(&topic.type_name))
                                        .small()
                                        .color(MUTED),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    badge(ui, &format!("D{}", topic.domain), color);
                                    ui.label(
                                        RichText::new(format!(
                                            "{} pub · {} sub",
                                            topic.publishers, topic.subscribers
                                        ))
                                        .small()
                                        .color(MUTED),
                                    );
                                    ui.label(
                                        RichText::new(if runtime_schema {
                                            "REFLECTED"
                                        } else if schema_ok {
                                            "EMBEDDED"
                                        } else {
                                            "RAW"
                                        })
                                        .small()
                                        .color(if schema_ok { MINT } else { AMBER }),
                                    );
                                });
                            })
                            .response;
                        if ui
                            .interact(
                                response.rect,
                                ui.id().with(&topic.key),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.reset_sample();
                            self.selected = Some((*topic).clone());
                            if let Some(c) = &self.connection {
                                c.select_with_reflection(Some((*topic).clone()), !embedded_schema);
                            }
                        }
                        ui.add_space(3.0);
                    }
                    if visible.is_empty() {
                        ui.add_space(20.0);
                        ui.label(
                            RichText::new(if !connected {
                                "Connect to explore the ROS graph."
                            } else if topics.is_empty() {
                                "Listening for advertised ROS topics…"
                            } else {
                                "No topics match this domain or search."
                            })
                            .color(MUTED),
                        );
                    }
                });
            });
        egui::CentralPanel::default().show(root_ui, |ui| {
            let Some(topic) = &self.selected else {
                ui.add_space(65.0);
                mark(ui, 64.0);
                ui.heading("Explore the ROS graph.");
                ui.label("Discover domains. Follow a topic. Inspect every field.");
                ui.add_space(20.0);
                ui.horizontal_wrapped(|ui| {
                    metric(ui, "DOMAINS", domains.len().to_string(), CYAN);
                    metric(ui, "TOPICS", topics.len().to_string(), MINT);
                    metric(ui, "SCHEMAS", self.registry.len().to_string(), LILAC);
                });
                ui.add_space(20.0);
                ui.label("Select a topic on the left to inspect its latest message.");
                ui.add_space(14.0);
                ui.weak("Nested fields and arrays are decoded from embedded or runtime schemas.");
                ui.weak("Live view keeps the latest sample. Pause freezes the view while counters continue.");
                return;
            };
            ui.add_space(14.0);
            badge(ui, &format!("DOMAIN {}", topic.domain), domain_color(topic.domain));
            ui.label(RichText::new(&topic.name).size(25.0).strong());
            ui.label(display_type(&topic.type_name));
            if !topics.iter().any(|t| t.key == topic.key && t.publishers > 0) { ui.colored_label(Color32::YELLOW, "No publisher currently advertised for this topic."); }
            ui.horizontal_wrapped(|ui| {
                metric(ui, "FREQUENCY", format!("{:.1} Hz", self.rate.0), CYAN);
                metric(ui, "THROUGHPUT", format!("{:.1} KiB/s", self.rate.1 / 1024.0), MINT);
                metric(ui, "RECEIVED", counts.0.to_string(), LILAC);
                if let Some(sample) = &self.shown { metric(ui, "LATEST", format!("{} B · {:.1}s", sample.size, sample.received.elapsed().as_secs_f64()), AMBER); }
                ui.separator(); ui.toggle_value(&mut self.paused, "Pause view");
            });
            if let Some(error) = error { ui.colored_label(Color32::LIGHT_RED, error); }
            if reflection_pending { ui.colored_label(CYAN, "Fetching runtime type description…"); }
            if let Some(error) = &reflection_error { ui.colored_label(Color32::YELLOW, error); }
            let waiting = if !connected {
                "Disconnected. Reconnect to receive data."
            } else if !subscription_ready || active_key.as_ref() != Some(&topic.key) {
                "Subscribing…"
            } else if !topics.iter().any(|t| t.key == topic.key && t.publishers > 0) {
                "No publisher is currently advertised. Listening for one to appear."
            } else if topics.iter().any(|t| t.key == topic.key && t.transient_local) {
                "Waiting for a retained sample or the next publication…"
            } else {
                "Listening. This topic sends new publications only; it has no retained history."
            };
            ui.separator();
            ui.horizontal(|ui| {
                for (i, name) in ["Overview", "Fields", "Raw", "Schema"].iter().enumerate() { ui.selectable_value(&mut self.tab, i, *name); }
            });
            ui.separator();
            egui::ScrollArea::both().id_salt((&topic.key, self.tab)).show(ui, |ui| {
                if self.tab == 3 {
                    ui.strong("Wire identity"); ui.monospace(&topic.key); ui.monospace(&topic.hash);
                    ui.add_space(10.0);
                    if let Some(live) = topics.iter().find(|t| t.key == topic.key) { for node in &live.nodes { ui.label(format!("Node: {node}")); } }
                    let schema = reflected_schema.clone().map(Ok)
                        .unwrap_or_else(|| self.registry.get(&topic.type_name, &topic.hash));
                    match schema {
                        Ok(schema) => for field in &schema.fields { ui.horizontal(|ui| { ui.label(&field.name); ui.monospace(views::type_label(&field.field_type)); }); },
                        Err(e) => { ui.colored_label(Color32::YELLOW, e); }
                    }
                } else if self.tab == 2 {
                    if let Some(sample) = &self.shown {
                        if sample.size > MAX_PAYLOAD {
                            ui.colored_label(Color32::YELLOW, format!("{} bytes received; payload exceeds the 8 MiB retention limit.", sample.size));
                        } else { views::hex_view(ui, &sample.bytes); }
                    }
                    else { ui.weak(waiting); }
                } else {
                    match &self.decoded {
                        Some(Ok(message)) => {
                            if self.tab == 1 || !views::custom(ui, message) { views::fields(ui, message); }
                        }
                        Some(Err(e)) => { ui.colored_label(Color32::YELLOW, e); ui.label("The Raw tab shows available payload bytes."); }
                        None => { ui.weak(waiting); }
                    }
                }
            });
        });
    }
}

const CYAN: Color32 = Color32::from_rgb(70, 211, 241);
const MINT: Color32 = Color32::from_rgb(90, 226, 172);
const LILAC: Color32 = Color32::from_rgb(181, 156, 255);
const AMBER: Color32 = Color32::from_rgb(255, 196, 99);
const MUTED: Color32 = Color32::from_rgb(147, 172, 204);

fn domain_color(domain: usize) -> Color32 {
    [CYAN, LILAC, MINT, AMBER, Color32::from_rgb(255, 139, 169)][(domain ^ (domain >> 3)) % 5]
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(color));
        });
}

fn metric(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
    egui::Frame::new()
        .fill(Color32::from_rgb(18, 34, 57))
        .corner_radius(8)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_min_width(104.0);
                ui.label(RichText::new(label).small().color(MUTED));
                ui.label(RichText::new(value).size(22.0).strong().color(color));
            });
        });
}

fn mark(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    for offset in [
        egui::vec2(-0.3, -0.3),
        egui::vec2(0.3, -0.3),
        egui::vec2(0.0, 0.32),
    ] {
        let point = center + offset * size;
        painter.line_segment([center, point], egui::Stroke::new(2.0, CYAN));
        painter.circle_filled(point, size * 0.09, MINT);
    }
    painter.circle_filled(center, size * 0.13, CYAN);
}
