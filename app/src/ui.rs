//! Small building blocks shared by the views.

use std::path::PathBuf;

use gpui_kit::{
    App,
    ClipboardItem,
    Context,
    Div,
    Entity,
    IntoElement,
    ParentElement,
    PathPromptOptions,
    SharedString,
    Styled,
    Window,
    component::{
        ActiveTheme,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Textarea, TextareaState},
        v_flex,
    },
    div,
    prelude::*,
    px,
};

pub fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_md()
        .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(title.into()))
}

pub fn row(label: impl Into<SharedString>, value: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(150.))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(value))
}

pub fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn error(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_color(cx.theme().danger).child(text.into())
}

pub fn success(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_color(cx.theme().success).child(text.into())
}

pub fn mono(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .child(text.into())
}

/// A read-only textarea holding an armored block, with Copy and Save buttons.
pub fn armored_block<T: 'static>(
    id: &'static str,
    state: &Entity<TextareaState>,
    file_name: String,
    cx: &mut Context<T>,
) -> Div {
    let text = state.read(cx).value().to_string();
    let copy_text = text.clone();
    v_flex().gap_2().child(Textarea::new(state).readonly(true)).child(
        h_flex()
            .gap_2()
            .child(
                Button::new(SharedString::from(format!("{id}-copy")))
                    .primary()
                    .label("Copy")
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()))),
            )
            .child(
                Button::new(SharedString::from(format!("{id}-save")))
                    .label("Save to file…")
                    .on_click(move |_, _, cx| save_to_file(text.clone(), file_name.clone(), cx)),
            ),
    )
}

pub fn new_textarea(rows: usize, placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<TextareaState> {
    let placeholder = placeholder.to_string();
    cx.new(|cx| TextareaState::new(window, cx).rows(rows).placeholder(placeholder))
}

pub fn set_text(state: &Entity<TextareaState>, text: String, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, cx| state.set_value(text, window, cx));
}

fn save_to_file(text: String, file_name: String, cx: &mut App) {
    let directory = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let prompt = cx.prompt_for_new_path(&directory, Some(&file_name));
    cx.spawn(async move |_| {
        if let Ok(Ok(Some(path))) = prompt.await {
            let _ = std::fs::write(path, text);
        }
    })
    .detach();
}

/// Asks for a file and resolves with its text, or `None` if cancelled or unreadable.
pub async fn open_file(cx: &gpui_kit::AsyncApp) -> Option<String> {
    let prompt = cx.update(|cx| {
        cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        })
    });
    let path = prompt.await.ok()?.ok()??.into_iter().next()?;
    std::fs::read_to_string(path).ok()
}
