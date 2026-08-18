//! Action definitions and key bindings for both block editing and app-level
//! window/menu commands.
//!
//! Text-editing actions are scoped to the `"BlockEditor"` key context on each
//! block. Window and menu commands use global bindings so they remain
//! available even when focus is on non-block UI such as dialogs or buttons.

use std::collections::{BTreeMap, BTreeSet};

use gpui::*;
use schemars::JsonSchema;
use serde::Deserialize;

actions!(
    velotype,
    [
        Newline,
        DeleteBack,
        Delete,
        WordDeleteBack,
        WordDeleteForward,
        FocusPrev,
        FocusNext,
        MoveLeft,
        MoveRight,
        WordMoveLeft,
        WordMoveRight,
        Home,
        End,
        BlockUp,
        BlockDown,
        PageUp,
        PageDown,
        JumpToTop,
        JumpToBottom,
        SelectLeft,
        SelectRight,
        WordSelectLeft,
        WordSelectRight,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        BoldSelection,
        ItalicSelection,
        UnderlineSelection,
        StrikethroughSelection,
        HighlightSelection,
        SuperscriptSelection,
        SubscriptSelection,
        CodeSelection,
        IndentBlock,
        OutdentBlock,
        Heading1,
        Heading2,
        Heading3,
        Heading4,
        Heading5,
        Heading6,
        ConvertToParagraph,
        ConvertToCodeBlock,
        ConvertToBulletedList,
        ConvertToNumberedList,
        ConvertToTaskList,
        ConvertToQuote,
        ExitCodeBlock,
        InsertLink,
        InsertImage,
        InsertMedia,
        InsertHorizontalRule,
        InsertTable,
        InsertTableRowBelow,
        InsertTableColumnRight,
        DeleteTable,
        Find,
        FindNext,
        FindPrevious,
        Replace,
        ReplaceAll,
        PastePlainText,
        SaveDocument,
        NewWindow,
        OpenFile,
        OpenPreferences,
        NoRecentFiles,
        SaveDocumentAs,
        ExportHtml,
        ExportPdf,
        AddLanguageConfig,
        AddThemeConfig,
        QuitApplication,
        CloseWindow,
        CheckForUpdates,
        ShowAbout,
        InstallCliTool,
        UninstallCliTool,
        DismissTransientUi,
        ToggleViewMode,
        ToggleWorkspace,
    ]
);

/// Selects a theme from the app-level theme registry.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, gpui::Action)]
#[action(namespace = velotype)]
#[serde(deny_unknown_fields)]
pub struct SelectTheme {
    /// Stable theme id from the built-in theme catalog.
    pub theme_id: String,
}

/// Selects a UI language from the app-level language registry.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, gpui::Action)]
#[action(namespace = velotype)]
#[serde(deny_unknown_fields)]
pub struct SelectLanguage {
    /// Stable language id from the built-in language catalog.
    pub language_id: String,
}

/// Opens a previously recorded Markdown file path.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, gpui::Action)]
#[action(namespace = velotype)]
#[serde(deny_unknown_fields)]
pub struct OpenRecentFile {
    /// Path stored in Velotype's recent-file history.
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutCategory {
    File,
    Edit,
    Navigation,
    /// Markdown editor / notes block formatting shortcuts (headings, lists,
    /// bold/italic, code blocks, etc.). Shared by both the Notes panel and the
    /// standalone Markdown editor since they use the same WYSIWYG Editor.
    Markdown,
    Block,
    Other,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutCommand {
    Newline,
    DeleteBack,
    Delete,
    WordDeleteBack,
    WordDeleteForward,
    FocusPrev,
    FocusNext,
    MoveLeft,
    MoveRight,
    WordMoveLeft,
    WordMoveRight,
    Home,
    End,
    BlockUp,
    BlockDown,
    PageUp,
    PageDown,
    JumpToTop,
    JumpToBottom,
    SelectLeft,
    SelectRight,
    WordSelectLeft,
    WordSelectRight,
    SelectHome,
    SelectEnd,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    BoldSelection,
    ItalicSelection,
    UnderlineSelection,
    StrikethroughSelection,
    HighlightSelection,
    SuperscriptSelection,
    SubscriptSelection,
    CodeSelection,
    IndentBlock,
    OutdentBlock,
    ExitCodeBlock,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    ConvertToParagraph,
    ConvertToCodeBlock,
    ConvertToBulletedList,
    ConvertToNumberedList,
    ConvertToTaskList,
    ConvertToQuote,
    InsertLink,
    InsertImage,
    InsertHorizontalRule,
    InsertTable,
    InsertTableRowBelow,
    InsertTableColumnRight,
    DeleteTable,
    Find,
    FindNext,
    FindPrevious,
    Replace,
    ReplaceAll,
    PastePlainText,
    SaveDocument,
    SaveDocumentAs,
    NewWindow,
    OpenFile,
    QuitApplication,
    CloseWindow,
    DismissTransientUi,
    ToggleViewMode,
    ToggleWorkspace,
}

#[derive(Clone, Copy, Debug)]
pub struct ShortcutDefinition {
    pub command: ShortcutCommand,
    pub id: &'static str,
    pub category: ShortcutCategory,
    pub default_keys: &'static [&'static str],
    pub context: Option<&'static str>,
}

const BLOCK_CONTEXT: Option<&str> = Some("BlockEditor");
/// Key context active while the in-editor find bar holds focus. Find-next /
/// find-previous bindings live here so Enter / arrow keys inside the search
/// field can drive navigation without conflicting with block editing.
const FIND_BAR_CONTEXT: Option<&str> = Some("EditorFindBar");
const SELECT_ALL_ID: &str = "select_all";
const LEGACY_SELECT_ALL_IDS: &[&str] = &[
    "select_all_source_text",
    "select_focused_block_text_rendered",
];

// On macOS cmd-q is the system quit shortcut; Windows/Linux use Alt+F4 (OS-handled).
#[cfg(target_os = "macos")]
const QUIT_APPLICATION_DEFAULT_KEYS: &[&str] = &["cmd-q"];
#[cfg(not(target_os = "macos"))]
const QUIT_APPLICATION_DEFAULT_KEYS: &[&str] = &[];

// On macOS cmd-w closes the current window; no app-level binding needed on other platforms.
#[cfg(target_os = "macos")]
const CLOSE_WINDOW_DEFAULT_KEYS: &[&str] = &["cmd-w"];
#[cfg(not(target_os = "macos"))]
const CLOSE_WINDOW_DEFAULT_KEYS: &[&str] = &["ctrl-q"];

const SHORTCUT_DEFINITIONS: &[ShortcutDefinition] = &[
    ShortcutDefinition {
        command: ShortcutCommand::Newline,
        id: "newline",
        category: ShortcutCategory::Block,
        default_keys: &["enter"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::DeleteBack,
        id: "delete_back",
        category: ShortcutCategory::Edit,
        default_keys: &["backspace"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Delete,
        id: "delete",
        category: ShortcutCategory::Edit,
        default_keys: &["delete"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordDeleteBack,
        id: "word_delete_back",
        category: ShortcutCategory::Edit,
        default_keys: &["ctrl-backspace", "alt-backspace"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordDeleteForward,
        id: "word_delete_forward",
        category: ShortcutCategory::Edit,
        default_keys: &["ctrl-delete", "alt-delete"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::FocusPrev,
        id: "focus_prev",
        category: ShortcutCategory::Navigation,
        default_keys: &["up"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::FocusNext,
        id: "focus_next",
        category: ShortcutCategory::Navigation,
        default_keys: &["down"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::MoveLeft,
        id: "move_left",
        category: ShortcutCategory::Navigation,
        default_keys: &["left"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::MoveRight,
        id: "move_right",
        category: ShortcutCategory::Navigation,
        default_keys: &["right"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordMoveLeft,
        id: "word_move_left",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-left", "alt-left"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordMoveRight,
        id: "word_move_right",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-right", "alt-right"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Home,
        id: "home",
        category: ShortcutCategory::Navigation,
        default_keys: &["home"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::End,
        id: "end",
        category: ShortcutCategory::Navigation,
        default_keys: &["end"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::BlockUp,
        id: "block_up",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-up", "alt-up"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::BlockDown,
        id: "block_down",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-down", "alt-down"],
        context: BLOCK_CONTEXT,
    },
    // Page scroll and document jumps operate on the editor viewport rather than
    // a single block, so they use global bindings (no context) and stay active
    // in both Rendered and Source mode.
    ShortcutDefinition {
        command: ShortcutCommand::PageUp,
        id: "page_up",
        category: ShortcutCategory::Navigation,
        default_keys: &["pageup"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::PageDown,
        id: "page_down",
        category: ShortcutCategory::Navigation,
        default_keys: &["pagedown"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::JumpToTop,
        id: "jump_to_top",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-home", "cmd-up"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::JumpToBottom,
        id: "jump_to_bottom",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-end", "cmd-down"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SelectLeft,
        id: "select_left",
        category: ShortcutCategory::Navigation,
        default_keys: &["shift-left"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SelectRight,
        id: "select_right",
        category: ShortcutCategory::Navigation,
        default_keys: &["shift-right"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordSelectLeft,
        id: "word_select_left",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-shift-left", "alt-shift-left"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::WordSelectRight,
        id: "word_select_right",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-shift-right", "alt-shift-right"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SelectHome,
        id: "select_home",
        category: ShortcutCategory::Navigation,
        default_keys: &["shift-home"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SelectEnd,
        id: "select_end",
        category: ShortcutCategory::Navigation,
        default_keys: &["shift-end"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SelectAll,
        id: SELECT_ALL_ID,
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-a", "ctrl-a"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Copy,
        id: "copy",
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-c", "ctrl-c"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Cut,
        id: "cut",
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-x", "ctrl-x"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Paste,
        id: "paste",
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-v", "ctrl-v"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Undo,
        id: "undo",
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-z", "ctrl-z"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Redo,
        id: "redo",
        category: ShortcutCategory::Edit,
        default_keys: &["cmd-shift-z", "ctrl-y"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::BoldSelection,
        id: "bold_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-b", "ctrl-b"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ItalicSelection,
        id: "italic_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-i", "ctrl-i"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::UnderlineSelection,
        id: "underline_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-u", "ctrl-u"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::StrikethroughSelection,
        id: "strikethrough_selection",
        category: ShortcutCategory::Markdown,
        // Alt + Shift + 5 is the cross-platform convention (Typora/Obsidian),
        // identical on macOS and Windows/Linux.
        default_keys: &["alt-shift-5"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::HighlightSelection,
        id: "highlight_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-=", "ctrl-="],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SuperscriptSelection,
        id: "superscript_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-.", "ctrl-shift-."],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SubscriptSelection,
        id: "subscript_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-,", "ctrl-shift-,"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::CodeSelection,
        id: "code_selection",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-e", "ctrl-e"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::IndentBlock,
        id: "indent_block",
        category: ShortcutCategory::Block,
        default_keys: &["tab"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::OutdentBlock,
        id: "outdent_block",
        category: ShortcutCategory::Block,
        default_keys: &["shift-tab"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ExitCodeBlock,
        id: "exit_code_block",
        category: ShortcutCategory::Block,
        default_keys: &["cmd-enter", "ctrl-enter"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading1,
        id: "heading_1",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-1", "ctrl-1"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading2,
        id: "heading_2",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-2", "ctrl-2"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading3,
        id: "heading_3",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-3", "ctrl-3"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading4,
        id: "heading_4",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-4", "ctrl-4"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading5,
        id: "heading_5",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-5", "ctrl-5"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Heading6,
        id: "heading_6",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-6", "ctrl-6"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToParagraph,
        id: "convert_to_paragraph",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-0", "ctrl-0"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToCodeBlock,
        id: "convert_to_code_block",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-k", "ctrl-shift-k"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToBulletedList,
        id: "convert_to_bulleted_list",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-u", "ctrl-shift-u"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToNumberedList,
        id: "convert_to_numbered_list",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-o", "ctrl-shift-o"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToTaskList,
        id: "convert_to_task_list",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-x", "ctrl-shift-x"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ConvertToQuote,
        id: "convert_to_quote",
        category: ShortcutCategory::Markdown,
        // Cmd/Ctrl+Shift+Q clashes with the macOS system Shift-Cmd-Q (force
        // quit / log out). Cmd+Option+Q / Ctrl+Alt+Q avoids that and the app
        // quit (Cmd+Q) binding.
        default_keys: &["cmd-alt-q", "ctrl-alt-q"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertLink,
        id: "insert_link",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-k", "ctrl-k"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertImage,
        id: "insert_image",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-i", "ctrl-shift-i"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertHorizontalRule,
        id: "insert_horizontal_rule",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift--", "ctrl-shift--"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertTable,
        id: "insert_table",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-t", "ctrl-t"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertTableRowBelow,
        id: "insert_table_row_below",
        category: ShortcutCategory::Markdown,
        // Alt-Enter mirrors the spreadsheet convention for "insert row"; kept
        // distinct from cmd-enter (ExitCodeBlock) and ctrl-enter to avoid
        // binding collisions inside the BlockEditor context.
        default_keys: &["alt-enter", "ctrl-alt-enter"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::InsertTableColumnRight,
        id: "insert_table_column_right",
        category: ShortcutCategory::Markdown,
        default_keys: &["alt-shift-enter", "ctrl-alt-shift-enter"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::DeleteTable,
        id: "delete_table",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-backspace", "ctrl-shift-backspace"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Find,
        id: "find",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-f", "ctrl-f"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::FindNext,
        id: "find_next",
        category: ShortcutCategory::Markdown,
        // Cmd-G on macOS, F3 elsewhere; both fire only while the find bar
        // holds focus (EditorFindBar context).
        default_keys: &["cmd-g", "f3"],
        context: FIND_BAR_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::FindPrevious,
        id: "find_previous",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-g", "shift-f3"],
        context: FIND_BAR_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::Replace,
        id: "replace",
        category: ShortcutCategory::Markdown,
        // Bound at the block level (not the find-bar context) so Cmd/Ctrl+H
        // opens the find/replace bar directly from the editor without first
        // requiring Cmd+F. The handler enables replace mode and focuses the
        // replacement field.
        default_keys: &["cmd-h", "ctrl-h"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ReplaceAll,
        id: "replace_all",
        category: ShortcutCategory::Markdown,
        // No default keybinding: triggered from the find/replace bar's UI to
        // avoid clashing with common editor bindings.
        default_keys: &[],
        context: FIND_BAR_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::PastePlainText,
        id: "paste_plain_text",
        category: ShortcutCategory::Markdown,
        default_keys: &["cmd-shift-v", "ctrl-shift-v"],
        context: BLOCK_CONTEXT,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SaveDocument,
        id: "save_document",
        category: ShortcutCategory::File,
        default_keys: &["cmd-s", "ctrl-s"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::SaveDocumentAs,
        id: "save_document_as",
        category: ShortcutCategory::File,
        default_keys: &["cmd-shift-s", "ctrl-shift-s"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::NewWindow,
        id: "new_window",
        category: ShortcutCategory::File,
        default_keys: &["cmd-n", "ctrl-n"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::OpenFile,
        id: "open_file",
        category: ShortcutCategory::File,
        default_keys: &["cmd-o", "ctrl-o"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::QuitApplication,
        id: "quit_application",
        category: ShortcutCategory::File,
        default_keys: QUIT_APPLICATION_DEFAULT_KEYS,
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::CloseWindow,
        id: "close_window",
        category: ShortcutCategory::File,
        default_keys: CLOSE_WINDOW_DEFAULT_KEYS,
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::DismissTransientUi,
        id: "dismiss_transient_ui",
        category: ShortcutCategory::Other,
        default_keys: &["escape"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ToggleViewMode,
        id: "toggle_view_mode",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-tab", "cmd-tab"],
        context: None,
    },
    ShortcutDefinition {
        command: ShortcutCommand::ToggleWorkspace,
        id: "toggle_workspace",
        category: ShortcutCategory::Navigation,
        default_keys: &["ctrl-w"],
        context: None,
    },
];

pub fn shortcut_definitions() -> &'static [ShortcutDefinition] {
    SHORTCUT_DEFINITIONS
}

/// Human-readable label for a shortcut id, for the Settings → Shortcuts list.
/// Falls back to the id itself for ids without an explicit label.
pub fn shortcut_label(id: &str) -> &'static str {
    match id {
        "newline" => "New line",
        "delete_back" => "Delete (back)",
        "delete" => "Delete (forward)",
        "word_delete_back" => "Delete word (back)",
        "word_delete_forward" => "Delete word (forward)",
        "focus_prev" => "Previous block",
        "focus_next" => "Next block",
        "move_left" => "Move left",
        "move_right" => "Move right",
        "word_move_left" => "Move word left",
        "word_move_right" => "Move word right",
        "home" => "Line start",
        "end" => "Line end",
        "block_up" => "Move up",
        "block_down" => "Move down",
        "page_up" => "Page up",
        "page_down" => "Page down",
        "jump_to_top" => "Jump to document top",
        "jump_to_bottom" => "Jump to document bottom",
        "select_left" => "Select left",
        "select_right" => "Select right",
        "word_select_left" => "Select word left",
        "word_select_right" => "Select word right",
        "select_home" => "Select to line start",
        "select_end" => "Select to line end",
        "select_all" => "Select all",
        "copy" => "Copy",
        "cut" => "Cut",
        "paste" => "Paste",
        "undo" => "Undo",
        "redo" => "Redo",
        "bold_selection" => "Bold",
        "italic_selection" => "Italic",
        "underline_selection" => "Underline",
        "strikethrough_selection" => "Strikethrough",
        "highlight_selection" => "Highlight",
        "superscript_selection" => "Superscript",
        "subscript_selection" => "Subscript",
        "code_selection" => "Inline code",
        "indent_block" => "Indent / nest",
        "outdent_block" => "Outdent / unnest",
        "exit_code_block" => "Exit code block",
        "heading_1" => "Heading 1",
        "heading_2" => "Heading 2",
        "heading_3" => "Heading 3",
        "heading_4" => "Heading 4",
        "heading_5" => "Heading 5",
        "heading_6" => "Heading 6",
        "convert_to_paragraph" => "Convert to paragraph",
        "convert_to_code_block" => "Convert to code block",
        "convert_to_bulleted_list" => "Convert to bulleted list",
        "convert_to_numbered_list" => "Convert to numbered list",
        "convert_to_task_list" => "Convert to task list",
        "convert_to_quote" => "Convert to blockquote",
        "insert_link" => "Insert link",
        "insert_image" => "Insert image",
        "insert_horizontal_rule" => "Horizontal rule",
        "insert_table" => "Insert table",
        "insert_table_row_below" => "Insert table row",
        "insert_table_column_right" => "Insert table column",
        "delete_table" => "Delete table",
        "find" => "Find",
        "find_next" => "Find next",
        "find_previous" => "Find previous",
        "replace" => "Find & replace",
        "replace_all" => "Replace all",
        "paste_plain_text" => "Paste as plain text",
        "toggle_view_mode" => "Toggle Rendered / Source",
        "toggle_workspace" => "Toggle outline drawer",
        _ => "",
    }
}

/// Display label for a shortcut category (section heading in Settings).
pub fn shortcut_category_label(category: ShortcutCategory) -> &'static str {
    match category {
        ShortcutCategory::File => "File",
        ShortcutCategory::Edit => "Edit",
        ShortcutCategory::Navigation => "Navigation",
        ShortcutCategory::Markdown => "Markdown",
        ShortcutCategory::Block => "Block",
        ShortcutCategory::Other => "Other",
    }
}

pub fn normalize_shortcut_keys(keys: &[String]) -> Option<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for key in keys {
        let parsed = Keystroke::parse(key.trim()).ok()?;
        if parsed.is_ime_in_progress() {
            return None;
        }
        let key = parsed.unparse();
        if seen.insert(key.clone()) {
            normalized.push(key);
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn default_keys(definition: ShortcutDefinition) -> Vec<String> {
    definition
        .default_keys
        .iter()
        .map(|key| key.to_string())
        .collect()
}

/// Legacy preference keys that should feed a modern shortcut definition.
///
/// Select-all used to be represented by separate source/rendered commands. The
/// editor now cycles those behaviors through one action, so old preferences map
/// forward to `select_all` instead of being silently discarded on load.
fn legacy_shortcut_ids(definition: ShortcutDefinition) -> &'static [&'static str] {
    match definition.command {
        ShortcutCommand::SelectAll => LEGACY_SELECT_ALL_IDS,
        _ => &[],
    }
}

/// Reads a user shortcut override, preferring the current id before aliases.
fn configured_shortcut_keys(
    definition: ShortcutDefinition,
    config: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    config
        .get(definition.id)
        .and_then(|keys| normalize_shortcut_keys(keys))
        .or_else(|| {
            legacy_shortcut_ids(definition).iter().find_map(|id| {
                config
                    .get(*id)
                    .and_then(|keys| normalize_shortcut_keys(keys))
            })
        })
}

fn shortcuts_conflict(
    left: ShortcutDefinition,
    left_keys: &[String],
    right: ShortcutDefinition,
    right_keys: &[String],
) -> bool {
    left.context == right.context && left_keys.iter().any(|key| right_keys.contains(key))
}

pub fn normalize_shortcut_config(
    config: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    let mut effective: BTreeMap<&'static str, (bool, Vec<String>)> = BTreeMap::new();
    for definition in SHORTCUT_DEFINITIONS {
        let custom = configured_shortcut_keys(*definition, config);
        effective.insert(
            definition.id,
            match custom {
                Some(keys) if keys != default_keys(*definition) => (true, keys),
                _ => (false, default_keys(*definition)),
            },
        );
    }

    loop {
        let mut conflicted = BTreeSet::new();
        for (index, left) in SHORTCUT_DEFINITIONS.iter().enumerate() {
            let (left_custom, left_keys) = effective.get(left.id).expect("known shortcut");
            for right in SHORTCUT_DEFINITIONS.iter().skip(index + 1) {
                let (right_custom, right_keys) = effective.get(right.id).expect("known shortcut");
                if shortcuts_conflict(*left, left_keys, *right, right_keys) {
                    if *left_custom {
                        conflicted.insert(left.id);
                    }
                    if *right_custom {
                        conflicted.insert(right.id);
                    }
                }
            }
        }

        if conflicted.is_empty() {
            break;
        }

        for id in conflicted {
            if let Some(definition) = SHORTCUT_DEFINITIONS
                .iter()
                .find(|definition| definition.id == id)
            {
                effective.insert(definition.id, (false, default_keys(*definition)));
            }
        }
    }

    effective
        .into_iter()
        .filter_map(|(id, (custom, keys))| custom.then_some((id.to_string(), keys)))
        .collect()
}

pub fn resolved_shortcut_keys(
    config: &BTreeMap<String, Vec<String>>,
    command: ShortcutCommand,
) -> Vec<String> {
    let normalized = normalize_shortcut_config(config);
    let definition = SHORTCUT_DEFINITIONS
        .iter()
        .find(|definition| definition.command == command)
        .expect("known shortcut command");
    normalized
        .get(definition.id)
        .cloned()
        .unwrap_or_else(|| default_keys(*definition))
}

pub fn shortcut_conflict_for(
    command: ShortcutCommand,
    proposed_keys: &[String],
    config: &BTreeMap<String, Vec<String>>,
) -> Option<ShortcutDefinition> {
    let definition = SHORTCUT_DEFINITIONS
        .iter()
        .find(|definition| definition.command == command)?;
    let proposed_keys = normalize_shortcut_keys(proposed_keys)?;
    for other in SHORTCUT_DEFINITIONS
        .iter()
        .filter(|other| other.command != command)
    {
        let other_keys = resolved_shortcut_keys(config, other.command);
        if shortcuts_conflict(*definition, &proposed_keys, *other, &other_keys) {
            return Some(*other);
        }
    }
    None
}

fn key_binding_for(
    command: ShortcutCommand,
    key: &str,
    context: Option<&'static str>,
) -> KeyBinding {
    match command {
        ShortcutCommand::Newline => KeyBinding::new(key, Newline, context),
        ShortcutCommand::DeleteBack => KeyBinding::new(key, DeleteBack, context),
        ShortcutCommand::Delete => KeyBinding::new(key, Delete, context),
        ShortcutCommand::WordDeleteBack => KeyBinding::new(key, WordDeleteBack, context),
        ShortcutCommand::WordDeleteForward => KeyBinding::new(key, WordDeleteForward, context),
        ShortcutCommand::FocusPrev => KeyBinding::new(key, FocusPrev, context),
        ShortcutCommand::FocusNext => KeyBinding::new(key, FocusNext, context),
        ShortcutCommand::MoveLeft => KeyBinding::new(key, MoveLeft, context),
        ShortcutCommand::MoveRight => KeyBinding::new(key, MoveRight, context),
        ShortcutCommand::WordMoveLeft => KeyBinding::new(key, WordMoveLeft, context),
        ShortcutCommand::WordMoveRight => KeyBinding::new(key, WordMoveRight, context),
        ShortcutCommand::Home => KeyBinding::new(key, Home, context),
        ShortcutCommand::End => KeyBinding::new(key, End, context),
        ShortcutCommand::BlockUp => KeyBinding::new(key, BlockUp, context),
        ShortcutCommand::BlockDown => KeyBinding::new(key, BlockDown, context),
        ShortcutCommand::PageUp => KeyBinding::new(key, PageUp, context),
        ShortcutCommand::PageDown => KeyBinding::new(key, PageDown, context),
        ShortcutCommand::JumpToTop => KeyBinding::new(key, JumpToTop, context),
        ShortcutCommand::JumpToBottom => KeyBinding::new(key, JumpToBottom, context),
        ShortcutCommand::SelectLeft => KeyBinding::new(key, SelectLeft, context),
        ShortcutCommand::SelectRight => KeyBinding::new(key, SelectRight, context),
        ShortcutCommand::WordSelectLeft => KeyBinding::new(key, WordSelectLeft, context),
        ShortcutCommand::WordSelectRight => KeyBinding::new(key, WordSelectRight, context),
        ShortcutCommand::SelectHome => KeyBinding::new(key, SelectHome, context),
        ShortcutCommand::SelectEnd => KeyBinding::new(key, SelectEnd, context),
        ShortcutCommand::SelectAll => KeyBinding::new(key, SelectAll, context),
        ShortcutCommand::Copy => KeyBinding::new(key, Copy, context),
        ShortcutCommand::Cut => KeyBinding::new(key, Cut, context),
        ShortcutCommand::Paste => KeyBinding::new(key, Paste, context),
        ShortcutCommand::Undo => KeyBinding::new(key, Undo, context),
        ShortcutCommand::Redo => KeyBinding::new(key, Redo, context),
        ShortcutCommand::BoldSelection => KeyBinding::new(key, BoldSelection, context),
        ShortcutCommand::ItalicSelection => KeyBinding::new(key, ItalicSelection, context),
        ShortcutCommand::UnderlineSelection => KeyBinding::new(key, UnderlineSelection, context),
        ShortcutCommand::StrikethroughSelection => {
            KeyBinding::new(key, StrikethroughSelection, context)
        }
        ShortcutCommand::HighlightSelection => KeyBinding::new(key, HighlightSelection, context),
        ShortcutCommand::SuperscriptSelection => {
            KeyBinding::new(key, SuperscriptSelection, context)
        }
        ShortcutCommand::SubscriptSelection => KeyBinding::new(key, SubscriptSelection, context),
        ShortcutCommand::CodeSelection => KeyBinding::new(key, CodeSelection, context),
        ShortcutCommand::IndentBlock => KeyBinding::new(key, IndentBlock, context),
        ShortcutCommand::OutdentBlock => KeyBinding::new(key, OutdentBlock, context),
        ShortcutCommand::ExitCodeBlock => KeyBinding::new(key, ExitCodeBlock, context),
        ShortcutCommand::Heading1 => KeyBinding::new(key, Heading1, context),
        ShortcutCommand::Heading2 => KeyBinding::new(key, Heading2, context),
        ShortcutCommand::Heading3 => KeyBinding::new(key, Heading3, context),
        ShortcutCommand::Heading4 => KeyBinding::new(key, Heading4, context),
        ShortcutCommand::Heading5 => KeyBinding::new(key, Heading5, context),
        ShortcutCommand::Heading6 => KeyBinding::new(key, Heading6, context),
        ShortcutCommand::ConvertToParagraph => KeyBinding::new(key, ConvertToParagraph, context),
        ShortcutCommand::ConvertToCodeBlock => KeyBinding::new(key, ConvertToCodeBlock, context),
        ShortcutCommand::ConvertToBulletedList => {
            KeyBinding::new(key, ConvertToBulletedList, context)
        }
        ShortcutCommand::ConvertToNumberedList => {
            KeyBinding::new(key, ConvertToNumberedList, context)
        }
        ShortcutCommand::ConvertToTaskList => KeyBinding::new(key, ConvertToTaskList, context),
        ShortcutCommand::ConvertToQuote => KeyBinding::new(key, ConvertToQuote, context),
        ShortcutCommand::InsertLink => KeyBinding::new(key, InsertLink, context),
        ShortcutCommand::InsertImage => KeyBinding::new(key, InsertImage, context),
        ShortcutCommand::InsertHorizontalRule => {
            KeyBinding::new(key, InsertHorizontalRule, context)
        }
        ShortcutCommand::InsertTable => KeyBinding::new(key, InsertTable, context),
        ShortcutCommand::InsertTableRowBelow => KeyBinding::new(key, InsertTableRowBelow, context),
        ShortcutCommand::InsertTableColumnRight => {
            KeyBinding::new(key, InsertTableColumnRight, context)
        }
        ShortcutCommand::DeleteTable => KeyBinding::new(key, DeleteTable, context),
        ShortcutCommand::Find => KeyBinding::new(key, Find, context),
        ShortcutCommand::FindNext => KeyBinding::new(key, FindNext, context),
        ShortcutCommand::FindPrevious => KeyBinding::new(key, FindPrevious, context),
        ShortcutCommand::Replace => KeyBinding::new(key, Replace, context),
        ShortcutCommand::ReplaceAll => KeyBinding::new(key, ReplaceAll, context),
        ShortcutCommand::PastePlainText => KeyBinding::new(key, PastePlainText, context),
        ShortcutCommand::SaveDocument => KeyBinding::new(key, SaveDocument, context),
        ShortcutCommand::SaveDocumentAs => KeyBinding::new(key, SaveDocumentAs, context),
        ShortcutCommand::NewWindow => KeyBinding::new(key, NewWindow, context),
        ShortcutCommand::OpenFile => KeyBinding::new(key, OpenFile, context),
        ShortcutCommand::QuitApplication => KeyBinding::new(key, QuitApplication, context),
        ShortcutCommand::CloseWindow => KeyBinding::new(key, CloseWindow, context),
        ShortcutCommand::DismissTransientUi => KeyBinding::new(key, DismissTransientUi, context),
        ShortcutCommand::ToggleViewMode => KeyBinding::new(key, ToggleViewMode, context),
        ShortcutCommand::ToggleWorkspace => KeyBinding::new(key, ToggleWorkspace, context),
    }
}

/// One editor shortcut exactly as it is registered in the embedded build:
/// the definition plus its effective context and key list (user overrides
/// and the chrome-toggle re-keying applied).
#[derive(Clone, Debug)]
pub struct EffectiveShortcut {
    pub definition: ShortcutDefinition,
    pub context: Option<&'static str>,
    pub keys: Vec<String>,
}

/// The editor shortcuts that are actually registered in the embedded verve
/// build: everything scoped to the block editor plus the viewport-scroll and
/// editor-chrome commands (re-scoped to `BlockEditor`). Entries whose
/// effective key list is empty (e.g. `replace_all`, reachable only from the
/// find/replace bar UI) are omitted. Shared by [`resolved_keybindings`] and
/// the help center so the documented list can never drift from the real
/// bindings.
pub fn effective_shortcuts(config: &BTreeMap<String, Vec<String>>) -> Vec<EffectiveShortcut> {
    let normalized = normalize_shortcut_config(config);
    let mut out = Vec::new();
    for definition in SHORTCUT_DEFINITIONS {
        // In the embedded verve build, skip window/application-level actions
        // (context == None) so they don't shadow verve's own global bindings
        // (cmd-s save, cmd-n new, cmd-q quit, cmd-w close, cmd-o open, ...).
        // verve owns those flows.
        //
        // EXCEPTIONS (also `context: None` in velotype, re-scoped to BlockEditor):
        // - Viewport-scroll (PageUp/PageDown/JumpToTop/JumpToBottom): scroll the
        //   editor, not the app window.
        // - Editor-chrome toggles (ToggleViewMode / ToggleWorkspace): toggle the
        //   editor's own Rendered↔Source mode and its outline drawer. Their
        //   default keys clash with OS/app shortcuts (cmd-tab, ctrl-w) so they
        //   are re-keyed below.
        if definition.context.is_none()
            && !is_viewport_navigation(definition.id)
            && !is_editor_chrome_toggle(definition.id)
        {
            continue;
        }
        let context = if definition.context.is_none() {
            BLOCK_CONTEXT
        } else {
            definition.context
        };
        // Re-key the chrome toggles to avoid clashes with the OS (cmd-tab) and
        // verve's own bindings (ctrl-w). User overrides via config still win.
        let keys = if is_editor_chrome_toggle(definition.id) {
            normalized.get(definition.id).cloned().unwrap_or_else(|| {
                remapped_chrome_keys(definition.id)
                    .iter()
                    .map(|k| (*k).to_string())
                    .collect()
            })
        } else {
            normalized
                .get(definition.id)
                .cloned()
                .unwrap_or_else(|| default_keys(*definition))
        };
        if keys.is_empty() {
            continue;
        }
        out.push(EffectiveShortcut {
            definition: *definition,
            context,
            keys,
        });
    }
    out
}

pub(crate) fn resolved_keybindings(config: &BTreeMap<String, Vec<String>>) -> Vec<KeyBinding> {
    effective_shortcuts(config)
        .into_iter()
        .flat_map(|shortcut| {
            let command = shortcut.definition.command;
            let context = shortcut.context;
            shortcut
                .keys
                .iter()
                .map(move |key| key_binding_for(command, key, context))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Default keys for the editor-chrome toggles in the embedded build, chosen to
/// avoid clashes with the OS (cmd-tab) and verve's own bindings (ctrl-w).
fn remapped_chrome_keys(id: &str) -> &'static [&'static str] {
    match id {
        "toggle_view_mode" => &["ctrl-shift-m"],
        "toggle_workspace" => &["cmd-\\"],
        _ => &[],
    }
}

/// Whether the given action id toggles editor chrome (view mode / workspace
/// drawer) rather than controlling the app window.
pub fn is_editor_chrome_toggle(id: &str) -> bool {
    matches!(id, "toggle_view_mode" | "toggle_workspace")
}

/// Whether the given action id is a viewport-scroll command that velotype
/// scopes globally but which should still be registered (re-scoped to the
/// BlockEditor context) in the embedded build.
pub fn is_viewport_navigation(id: &str) -> bool {
    matches!(
        id,
        "page_up" | "page_down" | "jump_to_top" | "jump_to_bottom"
    )
}

pub(crate) fn install_keybindings(cx: &mut App, config: &BTreeMap<String, Vec<String>>) {
    cx.bind_keys(resolved_keybindings(config));
}

/// Register key bindings for the block editor.
#[allow(dead_code)]
pub fn init(cx: &mut App) {
    install_keybindings(cx, &BTreeMap::new());
}

pub fn init_with_keybindings(cx: &mut App, config: &BTreeMap<String, Vec<String>>) {
    install_keybindings(cx, config);
}

#[cfg(test)]
mod tests {
    use super::{
        ShortcutCommand, normalize_shortcut_config, resolved_shortcut_keys, shortcut_conflict_for,
    };
    use std::collections::BTreeMap;

    #[test]
    fn custom_shortcut_replaces_command_defaults() {
        let mut config = BTreeMap::new();
        config.insert("save_document".to_string(), vec!["ctrl-alt-s".to_string()]);

        assert_eq!(
            resolved_shortcut_keys(&config, ShortcutCommand::SaveDocument),
            vec!["ctrl-alt-s".to_string()]
        );
    }

    #[test]
    fn toggle_view_mode_has_default_shortcuts() {
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::ToggleViewMode),
            vec!["ctrl-tab".to_string(), "cmd-tab".to_string()]
        );
    }

    #[test]
    fn toggle_workspace_has_default_shortcuts() {
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::ToggleWorkspace),
            vec!["ctrl-w".to_string()]
        );
    }

    #[test]
    fn select_all_has_default_shortcuts() {
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::SelectAll),
            vec!["cmd-a".to_string(), "ctrl-a".to_string()]
        );
        assert!(
            shortcut_conflict_for(
                ShortcutCommand::SelectAll,
                &["cmd-a".to_string(), "ctrl-a".to_string()],
                &BTreeMap::new()
            )
            .is_none()
        );
    }

    #[test]
    fn select_all_shortcut_can_be_customized() {
        let mut config = BTreeMap::new();
        config.insert("select_all".to_string(), vec!["ctrl-shift-a".to_string()]);

        assert_eq!(
            resolved_shortcut_keys(&config, ShortcutCommand::SelectAll),
            vec!["ctrl-shift-a".to_string()]
        );
    }

    #[test]
    fn legacy_split_select_all_shortcut_config_maps_to_unified_command() {
        let mut config = BTreeMap::new();
        config.insert(
            "select_all_source_text".to_string(),
            vec!["ctrl-shift-a".to_string()],
        );

        assert_eq!(
            resolved_shortcut_keys(&config, ShortcutCommand::SelectAll),
            vec!["ctrl-shift-a".to_string()]
        );

        let normalized = normalize_shortcut_config(&config);
        assert_eq!(
            normalized.get("select_all"),
            Some(&vec!["ctrl-shift-a".to_string()])
        );
        assert!(!normalized.contains_key("select_all_source_text"));
        assert!(!normalized.contains_key("select_focused_block_text_rendered"));

        config.clear();
        config.insert(
            "select_focused_block_text_rendered".to_string(),
            vec!["ctrl-alt-shift-a".to_string()],
        );

        assert_eq!(
            resolved_shortcut_keys(&config, ShortcutCommand::SelectAll),
            vec!["ctrl-alt-shift-a".to_string()]
        );

        let normalized = normalize_shortcut_config(&config);
        assert_eq!(
            normalized.get("select_all"),
            Some(&vec!["ctrl-alt-shift-a".to_string()])
        );
        assert!(!normalized.contains_key("select_all_source_text"));
        assert!(!normalized.contains_key("select_focused_block_text_rendered"));
    }

    #[test]
    fn close_and_quit_defaults_are_platform_specific() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::CloseWindow),
                vec!["cmd-w".to_string()]
            );
            assert_eq!(
                resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::QuitApplication),
                vec!["cmd-q".to_string()]
            );
        }

        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::CloseWindow),
                vec!["ctrl-q".to_string()]
            );
            assert!(
                resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::QuitApplication)
                    .is_empty()
            );
        }
    }

    #[test]
    fn word_and_block_shortcuts_have_ctrl_and_alt_defaults() {
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::WordMoveLeft),
            vec!["ctrl-left".to_string(), "alt-left".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::WordDeleteBack),
            vec!["ctrl-backspace".to_string(), "alt-backspace".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::BlockUp),
            vec!["ctrl-up".to_string(), "alt-up".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::WordSelectRight),
            vec![
                "ctrl-shift-right".to_string(),
                "alt-shift-right".to_string()
            ]
        );
    }

    #[test]
    fn page_navigation_shortcuts_have_defaults() {
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::PageUp),
            vec!["pageup".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::PageDown),
            vec!["pagedown".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::JumpToTop),
            vec!["ctrl-home".to_string(), "cmd-up".to_string()]
        );
        assert_eq!(
            resolved_shortcut_keys(&BTreeMap::new(), ShortcutCommand::JumpToBottom),
            vec!["ctrl-end".to_string(), "cmd-down".to_string()]
        );
    }

    #[test]
    fn invalid_or_empty_shortcuts_fall_back_to_defaults() {
        let mut config = BTreeMap::new();
        config.insert("save_document".to_string(), vec!["".to_string()]);
        config.insert("open_file".to_string(), vec!["a".to_string()]);

        let normalized = normalize_shortcut_config(&config);
        assert!(!normalized.contains_key("save_document"));
        assert!(!normalized.contains_key("open_file"));
    }

    #[test]
    fn conflicting_custom_shortcut_falls_back_to_default() {
        let mut config = BTreeMap::new();
        config.insert("copy".to_string(), vec!["ctrl-x".to_string()]);

        let normalized = normalize_shortcut_config(&config);
        assert!(!normalized.contains_key("copy"));
        assert_eq!(
            resolved_shortcut_keys(&config, ShortcutCommand::Copy),
            vec!["cmd-c".to_string(), "ctrl-c".to_string()]
        );
    }

    #[test]
    fn detects_shortcut_conflicts_for_preferences_drafts() {
        let conflict = shortcut_conflict_for(
            ShortcutCommand::Copy,
            &["ctrl-x".to_string()],
            &BTreeMap::new(),
        )
        .expect("copy should conflict with cut");

        assert_eq!(conflict.id, "cut");
    }
}
