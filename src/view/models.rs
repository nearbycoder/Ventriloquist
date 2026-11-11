//! Every model in the catalog: what it is, how big, and whether it is on this Mac. Nothing is
//! downloaded until it is asked for here, and anything downloaded can be removed again.

use rune::{El, ScrollSpec, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Loader};

use super::{avatar, dot, icons};
use crate::catalog::Kind;
use crate::state::{App, Install};
use crate::theme;

fn gb(size: f64) -> String {
    if size >= 10.0 { format!("{size:.0} GB") } else { format!("{size:.1} GB") }
}

/// The trailing end of a row: what can be done with the model in its current state.
fn action(s: &App, index: usize, k: &Kit) -> El {
    let c = &k.c;
    let m = &s.catalog.models[index];
    let id = &m.id;
    let line = row().items_center().gap(8.0);
    match &s.install[index] {
        Install::Missing => line.child(
            Button::new("Download")
                .variant(ButtonVariant::Neutral)
                .icon(&icons::DOWNLOAD)
                .help(format!("Build its Python environment and fetch {} of weights.", m.size))
                .on_press(move |s: &mut App| s.install(index))
                .build(k)
                .key(format!("download-{id}")),
        ),
        Install::Installing { .. } => line.child(Loader::new().build(k)).child(text("Downloading").font(k.ty.small).color(c.text_muted)).child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .help("Stop the download. What it fetched is kept, so starting again resumes.")
                .on_press(move |s: &mut App| s.cancel_install(index))
                .build(k)
                .key(format!("cancel-{id}")),
        ),
        Install::Ready if s.confirm_remove == Some(index) => line
            .child(Button::new("Keep").variant(ButtonVariant::Ghost).on_press(|s: &mut App| s.confirm_remove = None).build(k).key(format!("keep-{id}")))
            .child(
                Button::new(format!("Remove {}", m.size))
                    .variant(ButtonVariant::Danger)
                    .on_press(move |s: &mut App| s.remove(index))
                    .build(k)
                    .key(format!("confirm-remove-{id}")),
            ),
        Install::Ready => line.child(dot(theme::green(k))).child(text("Downloaded").font(k.ty.small).color(c.text_muted)).child(
            Button::new("Remove")
                .variant(ButtonVariant::Ghost)
                .help("Delete its weights from this Mac. You can download it again later.")
                .on_press(move |s: &mut App| s.confirm_remove = Some(index))
                .build(k)
                .key(format!("remove-{id}")),
        ),
        Install::Removing => line.child(Loader::new().build(k)).child(text("Removing").font(k.ty.small).color(c.text_muted)),
        Install::Failed { .. } => line.child(icon(&icons::WARNING, 14.0).color(theme::red(k))).child(
            Button::new("Try again").variant(ButtonVariant::Neutral).on_press(move |s: &mut App| s.install(index)).build(k).key(format!("retry-{id}")),
        ),
    }
}

fn model_row(s: &App, index: usize, k: &Kit) -> El {
    let c = &k.c;
    let m = &s.catalog.models[index];
    let mut facts = format!("{} · {} · {}", m.maker, m.released_label(), m.license);
    if !m.caution.is_empty() {
        facts = format!("{facts} ({})", m.caution.to_lowercase());
    }
    let mut body = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(3.0)
        .child(
            row()
                .items_center()
                .gap(8.0)
                .child(text(m.name.clone()).font(k.ty.ui_medium).color(c.text_base))
                .child(text(facts).font(k.ty.small).color(c.text_faint).truncate().grow(1.0)),
        )
        .child(text(m.summary.clone()).font(k.ty.small).color(c.text_muted).wrap().max_lines(2));
    // What the installer last said, while it runs or after it stopped short.
    match &s.install[index] {
        Install::Installing { log } => {
            let last = log.last().cloned().unwrap_or_else(|| "Starting".into());
            body = body.child(text(last).key(format!("progress-{}", m.id)).font(k.ty.mono).color(c.text_faint).truncate());
        }
        Install::Failed { log } => {
            let tail = log[log.len().saturating_sub(4)..].join("\n");
            body = body.child(text(tail).font(k.ty.mono).color(theme::red(k)).wrap().max_lines(6).selectable(true));
        }
        _ => {}
    }
    row()
        .key(format!("catalog-{}", m.id))
        .w_full()
        .items_center()
        .gap(14.0)
        .padding(12.0)
        .radius(k.radius.lg)
        .fill(theme::card(k))
        .border(1.0, c.border_weak)
        .child(avatar(m, 32.0, k))
        .child(body)
        .child(text(m.size.clone()).font(k.ty.mono).color(c.text_muted))
        .child(row().w(214.0).justify_end().child(action(s, index, k)))
}

pub fn models(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let total = s.catalog.models.len();
    let have = s.downloaded();
    let summary = match have {
        0 => "Nothing downloaded yet".to_owned(),
        _ => format!("{have} of {total} downloaded · {}", gb(s.size_where(Install::is_ready))),
    };
    let mut head = row().w_full().items_center().gap(12.0).child(
        text("Download the ones you want to try. Nothing is fetched until you ask, and everything runs on this Mac.")
            .font(k.ty.ui)
            .color(c.text_muted)
            .wrap()
            .grow(1.0),
    );
    head = head.child(text(summary).key("downloaded").font(k.ty.small).color(c.text_faint));
    let missing = s.missing();
    if missing > 1 {
        let rest = s.size_where(|i| matches!(i, Install::Missing | Install::Failed { .. }));
        head = head.child(
            Button::new(format!("Download all {missing}"))
                .variant(ButtonVariant::Ghost)
                .help(format!("Download every model not yet on this Mac: {} in all.", gb(rest)))
                .on_press(|s: &mut App| s.install_all())
                .build(k)
                .key("download-all"),
        );
    }
    let mut list = col().key("catalog").w_full().gap(8.0).px(20.0).pb(20.0);
    for kind in Kind::DOWNLOADABLE {
        let indices: Vec<usize> = (0..total).filter(|&i| s.catalog.models[i].kind == kind).collect();
        if indices.is_empty() {
            continue;
        }
        list = list.child(text(kind.title()).font(k.ty.small_medium).color(c.text_faint).mt(8.0));
        for i in indices {
            list = list.child(model_row(s, i, k));
        }
    }
    col().key("models-page").w_full().h_full().min_h(0.0).child(el().w_full().padding(20.0).pb(8.0).child(head)).child(k.scroll_bar(
        col().key("catalog-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(12.0).child(list),
    ))
}
