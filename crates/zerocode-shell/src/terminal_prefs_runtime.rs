use super::*;

/// Which task sources the person put away.
///
/// Its own store rather than a third name in `SHORTCUTS`: those are the two
/// rows above the project tree, and the strip that holds them disappears when
/// BOTH are away. Filing a task source in with them would tie that rule to a
/// thing that is not in the strip.
///
/// Orca's `Jira 숨기기` sits in the empty state itself, beside `Jira 연결` —
/// the two things you can do about a source that is not connected.
pub(super) fn stored_hidden_task_sources(legacy: &LegacySettings<'_>) -> Vec<String> {
    legacy
        .read(legacy_settings_file::HIDDEN_TASK_SOURCES)
        .unwrap_or_default()
}

/// Whether the sidebar is hiding the workspaces automations cut.
///
/// One of Orca's sidebar display options — `hideAutomationGeneratedWorkspaces`
/// sits in `sidebarHasActiveFilters` beside the repo filter and the
/// default-branch hide (index-ftls8Hg_.js:506679), so it belongs on the same
/// menu ours already has rather than in a settings screen. Off unless the file
/// says otherwise: a window that opens with workspaces missing and no memory
/// of being asked to hide them is a window that lost them.
/// Whether a coordinator may open panes for the agents it starts.
///
/// Orca's `claudeAgentTeamsMode` — Orca defaults it **off**
/// (out/main/index.js:3999). Ours defaults to **panes**: the person asked for
/// the split by name, twice ("소넷과 codex 다 소환하고 … 터미널창이 새로
/// 열리고 … 화면 분활이 이루어저야") — a recorded deviation, and the settings
/// picker still turns it off.
///
/// The pane road is a `sh` shim on `PATH` — the road Orca itself marks
/// unsupported on win32/wsl and falls to in-process
/// (`detectUnsupportedRuntimes`, `buildClaudeAgentTeamsLaunchPlan`). The same
/// fall here ("윈도우에서도 돌아가야함"): panes where panes can be, teammates
/// either way.
pub(super) fn stored_teams_mode(legacy: &LegacySettings<'_>) -> TeamsMode {
    let stored = legacy
        .read(legacy_settings_file::AGENT_TEAMS_MODE)
        .unwrap_or_default();
    if stored == TeamsMode::Panes && cfg!(not(unix)) {
        return TeamsMode::InProcess;
    }
    stored
}

pub(super) fn stored_hide_automation_workspaces(legacy: &LegacySettings<'_>) -> bool {
    legacy
        .read(legacy_settings_file::HIDE_AUTOMATION_WORKSPACES)
        .unwrap_or(false)
}

/// Whether closing a pinned tab asks first.
///
/// Orca's `confirmClosePinnedTab`, read `?? true` at the one place that reads
/// it (`guardPinnedTabClose`, index-ftls8Hg_.js:15406) — so the switch ships
/// ON and a machine that has never been asked behaves exactly as it did
/// before the switch existed. The absent file is that same `??`: the default
/// lives here, in one spelling, rather than being re-decided by every caller.
///
/// On is the safe direction for the same reason the pin exists at all. A pin
/// is a person saying "not this one"; a window that had forgotten the setting
/// and guessed OFF would close, without a word, the one tab that had been
/// marked as the one not to close.
pub(super) fn stored_confirm_close_pinned(legacy: &LegacySettings<'_>) -> bool {
    legacy
        .read(legacy_settings_file::CONFIRM_CLOSE_PINNED)
        .unwrap_or(true)
}

/// Whether a diff opens with the two versions beside each other.
///
/// Orca's `renderSideBySide` (DiffViewer-MmUuThJc.js:433), which is a person's
/// toggle and not a computed one — so it is remembered, and it ships ON for
/// the same reason Orca's does: side by side is the shape a review has when
/// nobody has said otherwise, and the other one is the answer to a narrow
/// window rather than the default way to read a change.
pub(super) fn stored_diff_side_by_side(legacy: &LegacySettings<'_>) -> bool {
    legacy
        .read(legacy_settings_file::DIFF_SIDE_BY_SIDE)
        .unwrap_or(true)
}

/// Which chords the person moved, and nothing else.
///
/// Sparse on purpose. Orca's store starts at `EMPTY_KEYBINDINGS = {}` and
/// holds `snapshot.overrides`, never the resolved table
/// (`store-BgJxB0hr.js:33313-33341`). The defaults stay in the action registry
/// the window already ships, so a chord nobody touched has nothing written for
/// it — and a default we improve later reaches everybody who never moved it.
pub(super) fn stored_keybindings(legacy: &LegacySettings<'_>) -> BTreeMap<String, Vec<String>> {
    legacy
        .read(legacy_settings_file::KEYBINDINGS)
        .unwrap_or_default()
}

/// The two shortcut rows above the project tree, by name.
///
/// Orca's are settings — `showTasksButton !== false` and
/// `showAutomationsButton !== false`, both on unless somebody turned them off
/// (`App-BaqTRjaA.js:8683-8807`, docs/reverse/orca-ui-inventory.md 1-k) — and
/// its row menu offers `Hide from sidebar`. So this is a stored preference
/// here too, and the name is the row's own id in the markup.
pub(super) const SHORTCUTS: &[&str] = &["tasks", "automations"];

/// Task sources whose backend is implemented and may be persisted.
///
/// This is the canonical allowlist used by normalization and both mutation
/// commands.
pub(super) const TASK_SOURCES: &[&str] = &["jira", "github", "gitlab", "linear"];

/// Which shortcut rows the person put away.
///
/// In the boot report rather than in the window's own storage for the reason
/// the theme is: a row that paints and then vanishes a frame later is a window
/// correcting itself in front of you. Unknown names are dropped rather than
/// kept, so a hand-edited file cannot hide a row this window has no way to
/// bring back.
pub(super) fn stored_hidden_shortcuts(legacy: &LegacySettings<'_>) -> Vec<String> {
    let kept: Vec<String> = legacy
        .read(legacy_settings_file::HIDDEN_SHORTCUTS)
        .unwrap_or_default();
    kept.into_iter()
        .filter(|name| SHORTCUTS.contains(&name.as_str()))
        .collect()
}

/// The treatments the tokens carry, plus the instruction to follow the OS.
///
/// `dark` is what `:root` binds and `light` is what `[data-theme="light"]`
/// binds (ui/tokens.css). `system` is not a treatment: it is the instruction
/// to let `prefers-color-scheme` pick between the two. The OS never drives
/// the window directly — it feeds this setting's `system` option, which is
/// the arrangement the token file was written for and says so.
pub(super) const THEMES: &[&str] = &["system", "dark", "light"];

/// The treatment the person chose, or `system` when they have not chosen.
pub(super) fn stored_theme(legacy: &LegacySettings<'_>) -> String {
    let kept: String = legacy.read(legacy_settings_file::THEME).unwrap_or_default();
    if THEMES.contains(&kept.as_str()) {
        kept
    } else {
        "system".to_string()
    }
}

/// The window's own material — Orca's Settings ▸ Window, both fields.
///
/// `terminal_opacity` is its `terminalBackgroundOpacity`: 1 is the screen as
/// it has always been, and anything below it lets what is behind the window
/// show through the terminal (`store-BgJxB0hr.js:21429`). `blur` is its
/// `windowBackgroundBlur`, which Orca turns into `backgroundMaterial:
/// "acrylic"` on win32 and into nothing at all on every other platform
/// (`createMainWindow`, out/main/index.js:205024) — so its own macOS users
/// have a switch that does nothing. Ours asks each platform for its own
/// material.
#[derive(Serialize, serde::Deserialize, Clone, Copy, PartialEq)]
pub(super) struct WindowMaterial {
    pub(super) terminal_opacity: f64,
    pub(super) blur: bool,
}

impl Default for WindowMaterial {
    fn default() -> Self {
        Self {
            // Orca's own defaults: opaque, no blur.
            terminal_opacity: 1.0,
            blur: false,
        }
    }
}

impl WindowMaterial {
    /// A stored file says whatever it says — hand-edited, or written by a
    /// version with different bounds. An opacity outside 0..=1 is clamped
    /// rather than refused, and a NaN falls back: neither is worth failing a
    /// boot over, and a window that opens invisible is worse than one that
    /// opens ignoring a preference.
    pub(super) fn clamped(self) -> Self {
        Self {
            terminal_opacity: if self.terminal_opacity.is_finite() {
                self.terminal_opacity.clamp(0.0, 1.0)
            } else {
                1.0
            },
            blur: self.blur,
        }
    }
}

pub(super) fn stored_window_material(legacy: &LegacySettings<'_>) -> WindowMaterial {
    let held = legacy
        .read::<WindowMaterial>(legacy_settings_file::WINDOW_MATERIAL)
        .unwrap_or_default();
    held.clamped()
}

/// How wide the two side columns are, in pixels.
///
/// Orca's `sidebarWidth: 280` and `rightSidebarWidth: 350` are **defaults a
/// person drags**, not constants — its UI store carries them and the drag
/// writes them back (docs/reverse/orca-ui-inventory.md 1-c). So they are a
/// setting here too, and the window reads them before it paints rather than
/// snapping to the stored width a frame later.
#[derive(Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq)]
pub(super) struct PanelWidths {
    pub(super) sidebar: u32,
    pub(super) aside: u32,
}

/// One independently draggable column. Sending only this item prevents a
/// renderer with a stale copy of the other column from putting it back.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PanelSide {
    Sidebar,
    Aside,
}

impl Default for PanelWidths {
    fn default() -> Self {
        Self {
            sidebar: 280,
            aside: 350,
        }
    }
}

/// The bounds a drag is held inside. Ours, not measured: the floor is the
/// width at which a worktree row stops being able to show its branch on the
/// second line, and the ceiling keeps a column from eating the stage. A
/// stored file that says otherwise — hand-edited, or written by a version
/// with different bounds — is clamped rather than refused, because a width
/// is not the sort of thing worth failing a boot over.
pub(super) const PANEL_WIDTH_BOUNDS: [(u32, u32); 2] = [(180, 560), (200, 640)];

impl PanelWidths {
    pub(super) fn clamped(self) -> Self {
        let [(sidebar_min, sidebar_max), (aside_min, aside_max)] = PANEL_WIDTH_BOUNDS;
        Self {
            sidebar: self.sidebar.clamp(sidebar_min, sidebar_max),
            aside: self.aside.clamp(aside_min, aside_max),
        }
    }

    pub(super) fn apply(&mut self, side: PanelSide, width: u32) {
        match side {
            PanelSide::Sidebar => self.sidebar = width,
            PanelSide::Aside => self.aside = width,
        }
        *self = self.clamped();
    }
}

pub(super) fn stored_panel_widths(legacy: &LegacySettings<'_>) -> PanelWidths {
    legacy
        .read::<PanelWidths>(legacy_settings_file::PANEL_WIDTHS)
        .unwrap_or_default()
        .clamped()
}

/// The terminal's own settings.
///
/// Orca exposes more than twenty `terminal*` options; this is the subset that
/// changes what a person SEES on the surface they stare at all day, and every
/// default here is the value the window already shipped — so a machine with no
/// file is byte-for-byte the window it had before this existed.
///
/// One record rather than one file per control: they are read together on every
/// boot and written together from one screen, and separate files would be more
/// chances for a half-written settings directory to describe a terminal nobody
/// chose.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalCursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalLigatureMode {
    #[default]
    Auto,
    On,
    Off,
}

/// Which macOS Option key becomes the terminal's Alt/Meta modifier.
///
/// The wire spellings deliberately match Orca 1.4.180. `true` and `false`
/// are strings in its settings document, not booleans; the enum keeps that
/// compatibility without letting the rest of the app become stringly typed.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum MacOptionAsAlt {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "true")]
    Both,
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "false")]
    Off,
}

/// Which PowerShell family a new local Windows terminal prefers.
///
/// The wire values match Orca 1.4.180. `Auto` notices a later PowerShell 7
/// install without rewriting settings; an explicit PowerShell 7 choice keeps
/// its intent but may fall back to Windows PowerShell if the executable was
/// removed after it was selected.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum WindowsPowerShellImplementation {
    #[default]
    Auto,
    #[serde(rename = "powershell.exe")]
    WindowsPowerShell,
    #[serde(rename = "pwsh.exe")]
    PowerShell7,
}

/// The shell family a new local Windows terminal opens.
///
/// These wire values are Orca 1.4.180's persisted contract. Git Bash remains
/// a symbolic choice because its executable path is machine-specific and is
/// resolved afresh when a pane opens.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum WindowsTerminalShell {
    #[default]
    #[serde(rename = "powershell.exe")]
    PowerShell,
    #[serde(rename = "cmd.exe")]
    CommandPrompt,
    #[serde(rename = "git-bash")]
    GitBash,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct TerminalThemeColors {
    pub(super) background: String,
    pub(super) foreground: String,
    pub(super) cursor: String,
    pub(super) cursor_accent: String,
    pub(super) selection_background: String,
    pub(super) selection_foreground: String,
    pub(super) black: String,
    pub(super) red: String,
    pub(super) green: String,
    pub(super) yellow: String,
    pub(super) blue: String,
    pub(super) magenta: String,
    pub(super) cyan: String,
    pub(super) white: String,
    pub(super) bright_black: String,
    pub(super) bright_red: String,
    pub(super) bright_green: String,
    pub(super) bright_yellow: String,
    pub(super) bright_blue: String,
    pub(super) bright_magenta: String,
    pub(super) bright_cyan: String,
    pub(super) bright_white: String,
}

pub(super) fn terminal_theme_catalog() -> &'static BTreeMap<String, TerminalThemeColors> {
    static CATALOG: OnceLock<BTreeMap<String, TerminalThemeColors>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("terminal_themes.json"))
            .expect("the built-in terminal theme catalog must stay valid")
    })
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub(super) enum TerminalColorKey {
    Foreground,
    Background,
    Cursor,
    CursorAccent,
    SelectionBackground,
    SelectionForeground,
    Bold,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl TerminalColorKey {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Cursor => "cursor",
            Self::CursorAccent => "cursorAccent",
            Self::SelectionBackground => "selectionBackground",
            Self::SelectionForeground => "selectionForeground",
            Self::Bold => "bold",
            Self::Black => "black",
            Self::Red => "red",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Cyan => "cyan",
            Self::White => "white",
            Self::BrightBlack => "brightBlack",
            Self::BrightRed => "brightRed",
            Self::BrightGreen => "brightGreen",
            Self::BrightYellow => "brightYellow",
            Self::BrightBlue => "brightBlue",
            Self::BrightMagenta => "brightMagenta",
            Self::BrightCyan => "brightCyan",
            Self::BrightWhite => "brightWhite",
        }
    }
}

#[derive(Clone, Deserialize)]
pub(super) struct TerminalColorOverridePatch {
    pub(super) key: TerminalColorKey,
    pub(super) value: Option<String>,
}

#[derive(Debug, Serialize, serde::Deserialize, Clone, PartialEq)]
#[serde(default)]
pub(super) struct TerminalPrefs {
    /// Cell size in px. Orca's `terminalFontSize`.
    pub(super) font_size: u32,
    /// The primary terminal face. Empty means this platform's Orca default.
    pub(super) font_family: String,
    /// Whether CSS `liga`/`calt` shaping follows the face, stays on, or stays off.
    pub(super) ligatures: TerminalLigatureMode,
    /// Whether either macOS Option key sends terminal Alt/Meta or composes text.
    pub(super) mac_option_as_alt: MacOptionAsAlt,
    /// Whether the unmodified macOS JIS Yen key sends a backslash.
    pub(super) jis_yen_to_backslash: bool,
    /// Whether terminal programs may write plain text through OSC 52.
    pub(super) allow_osc52_clipboard: bool,
    /// Which shell family new local Windows panes open.
    pub(super) windows_shell: WindowsTerminalShell,
    /// Which PowerShell executable new local Windows panes prefer.
    pub(super) windows_powershell_implementation: WindowsPowerShellImplementation,
    /// Built-in or imported terminal palette used by the dark app treatment.
    pub(super) theme_dark: String,
    /// Whether the light app treatment may select its own terminal palette.
    pub(super) use_separate_light_theme: bool,
    /// Built-in or imported palette used when the separate light treatment is on.
    pub(super) theme_light: String,
    /// Validated imported palettes. Their selection values are `custom:<id>`;
    /// built-ins keep their original catalog names.
    pub(super) custom_themes: Vec<CustomTerminalTheme>,
    /// Valid per-slot colors layered over either selected theme.
    pub(super) color_overrides: BTreeMap<String, String>,
    /// Row height as a multiple of the font size. 1.15 is Orca's
    /// `terminalLineHeight: 1` expressed in the unit CSS multiplies
    /// (docs/reverse/orca-ui-inventory.md 1-em) — the default is not 1.
    pub(super) leading: f32,
    /// Body weight. Orca's `terminalFontWeight`.
    pub(super) weight: u32,
    /// Whether the caret blinks. Orca's `cursorBlink`, default on.
    pub(super) cursor_blink: bool,
    /// The measured cell's cursor treatment.
    pub(super) cursor_style: TerminalCursorStyle,
    /// Opacity of the cursor's paint layer. Orca's `terminalCursorOpacity`.
    pub(super) cursor_opacity: f32,
    /// Horizontal padding around the terminal grid in px.
    pub(super) padding_x: u32,
    /// Vertical padding around the terminal grid in px.
    pub(super) padding_y: u32,
    /// Wheel sensitivity. Orca's `scrollSensitivity`.
    pub(super) sensitivity: f32,
    /// Lines of history kept per shell. Orca's terminal scrollback rows.
    pub(super) scrollback: usize,
    /// Characters that end a word when terminal text is double-clicked.
    pub(super) word_separators: String,
    /// Extra wheel travel while Alt is held.
    pub(super) fast_scroll_sensitivity: f32,
    /// Wheel travel reported to a program that owns terminal mouse input.
    pub(super) tui_scroll_sensitivity: u32,
    /// Pointing at a split pane gives it the keyboard without a click.
    pub(super) focus_follows_mouse: bool,
    /// Hide a terminal's pointer after input until the pointer moves again.
    pub(super) hide_mouse_while_typing: bool,
    /// Copy a completed selection from this terminal to the system clipboard.
    pub(super) copy_on_select: bool,
    /// Paste plain text from the system clipboard on a terminal right-click.
    pub(super) right_click_paste: bool,
    /// Opacity of a split pane that does not own the keyboard.
    pub(super) inactive_pane_opacity: f32,
    /// Split divider colour while the app uses its dark treatment.
    pub(super) divider_color_dark: String,
    /// Split divider colour while the app uses its light treatment.
    pub(super) divider_color_light: String,
    /// Visible terminal-pane divider thickness in px.
    pub(super) divider_thickness_px: u32,
}

/// A typed, single-control terminal patch.
///
/// Numeric fields intentionally use their concrete wire types. This keeps
/// validation and clamping in one place without a stringly typed field/value
/// pair or a bag of optional fields.
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum TerminalPrefsPatch {
    FontSize(u32),
    FontFamily(String),
    Ligatures(TerminalLigatureMode),
    MacOptionAsAlt(MacOptionAsAlt),
    JisYenToBackslash(bool),
    AllowOsc52Clipboard(bool),
    WindowsShell(WindowsTerminalShell),
    #[serde(rename = "windows_powershell_implementation")]
    WindowsPowerShellImplementation(WindowsPowerShellImplementation),
    ThemeDark(String),
    UseSeparateLightTheme(bool),
    ThemeLight(String),
    ImportCustomThemes(Vec<CustomTerminalTheme>),
    RemoveCustomTheme(String),
    ColorOverride(TerminalColorOverridePatch),
    ResetColorOverrides,
    Leading(f32),
    Weight(u32),
    CursorBlink(bool),
    CursorStyle(TerminalCursorStyle),
    CursorOpacity(f32),
    PaddingX(u32),
    PaddingY(u32),
    Sensitivity(f32),
    Scrollback(usize),
    WordSeparators(String),
    FastScrollSensitivity(f32),
    TuiScrollSensitivity(u32),
    FocusFollowsMouse(bool),
    HideMouseWhileTyping(bool),
    CopyOnSelect(bool),
    RightClickPaste(bool),
    InactivePaneOpacity(f32),
    DividerColorDark(String),
    DividerColorLight(String),
    DividerThicknessPx(u32),
}

impl TerminalPrefsPatch {
    pub(super) fn apply(self, prefs: &mut TerminalPrefs) {
        match self {
            Self::FontSize(value) => prefs.font_size = value,
            Self::FontFamily(value) => prefs.font_family = value,
            Self::Ligatures(value) => prefs.ligatures = value,
            Self::MacOptionAsAlt(value) => prefs.mac_option_as_alt = value,
            Self::JisYenToBackslash(value) => prefs.jis_yen_to_backslash = value,
            Self::AllowOsc52Clipboard(value) => prefs.allow_osc52_clipboard = value,
            Self::WindowsShell(value) => prefs.windows_shell = value,
            Self::WindowsPowerShellImplementation(value) => {
                prefs.windows_powershell_implementation = value;
            }
            Self::ThemeDark(value) => prefs.theme_dark = value,
            Self::UseSeparateLightTheme(value) => prefs.use_separate_light_theme = value,
            Self::ThemeLight(value) => prefs.theme_light = value,
            Self::ImportCustomThemes(themes) => {
                prefs.custom_themes =
                    merge_custom_themes(std::mem::take(&mut prefs.custom_themes), themes);
            }
            Self::RemoveCustomTheme(id) => {
                prefs.custom_themes =
                    remove_custom_theme(std::mem::take(&mut prefs.custom_themes), &id);
            }
            Self::ColorOverride(patch) => {
                let key = patch.key.as_str().to_string();
                if let Some(color) = patch
                    .value
                    .and_then(|value| normalized_optional_hex_color(&value))
                {
                    prefs.color_overrides.insert(key, color);
                } else {
                    prefs.color_overrides.remove(&key);
                }
            }
            Self::ResetColorOverrides => prefs.color_overrides.clear(),
            Self::Leading(value) => prefs.leading = value,
            Self::Weight(value) => prefs.weight = value,
            Self::CursorBlink(value) => prefs.cursor_blink = value,
            Self::CursorStyle(value) => prefs.cursor_style = value,
            Self::CursorOpacity(value) => prefs.cursor_opacity = value,
            Self::PaddingX(value) => prefs.padding_x = value,
            Self::PaddingY(value) => prefs.padding_y = value,
            Self::Sensitivity(value) => prefs.sensitivity = value,
            Self::Scrollback(value) => prefs.scrollback = value,
            Self::WordSeparators(value) => prefs.word_separators = value,
            Self::FastScrollSensitivity(value) => prefs.fast_scroll_sensitivity = value,
            Self::TuiScrollSensitivity(value) => prefs.tui_scroll_sensitivity = value,
            Self::FocusFollowsMouse(value) => prefs.focus_follows_mouse = value,
            Self::HideMouseWhileTyping(value) => prefs.hide_mouse_while_typing = value,
            Self::CopyOnSelect(value) => prefs.copy_on_select = value,
            Self::RightClickPaste(value) => prefs.right_click_paste = value,
            Self::InactivePaneOpacity(value) => prefs.inactive_pane_opacity = value,
            Self::DividerColorDark(value) => prefs.divider_color_dark = value,
            Self::DividerColorLight(value) => prefs.divider_color_light = value,
            Self::DividerThicknessPx(value) => prefs.divider_thickness_px = value,
        }
        *prefs = prefs.clone().clamped();
    }
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self {
            font_size: 14,
            font_family: String::new(),
            ligatures: TerminalLigatureMode::Auto,
            mac_option_as_alt: MacOptionAsAlt::Auto,
            jis_yen_to_backslash: false,
            allow_osc52_clipboard: true,
            windows_shell: WindowsTerminalShell::PowerShell,
            windows_powershell_implementation: WindowsPowerShellImplementation::Auto,
            theme_dark: TERM_THEME_DARK.to_string(),
            use_separate_light_theme: true,
            theme_light: TERM_THEME_LIGHT.to_string(),
            custom_themes: Vec::new(),
            color_overrides: BTreeMap::new(),
            leading: TERM_LINE_HEIGHT_BASE_LEADING,
            weight: 500,
            cursor_blink: true,
            cursor_style: TerminalCursorStyle::Block,
            cursor_opacity: 1.0,
            padding_x: 4,
            padding_y: 4,
            sensitivity: 1.15,
            scrollback: zerocode_pty::DEFAULT_SCROLLBACK_LINES,
            word_separators: DEFAULT_TERM_WORD_SEPARATORS.to_string(),
            fast_scroll_sensitivity: 5.0,
            tui_scroll_sensitivity: 1,
            focus_follows_mouse: false,
            hide_mouse_while_typing: false,
            copy_on_select: false,
            right_click_paste: false,
            inactive_pane_opacity: 0.9,
            divider_color_dark: TERM_DIVIDER_COLOR_DARK.to_string(),
            divider_color_light: TERM_DIVIDER_COLOR_LIGHT.to_string(),
            divider_thickness_px: 3,
        }
    }
}

/// What each settings row may offer.
///
/// Most bounds are also the storage envelope. Scroll speed deliberately keeps
/// Orca's wider compatibility envelope for restored/custom files while this
/// spec exposes only the range its visible sliders offer.
pub(super) const TERM_FONT_SIZE_BOUNDS: (u32, u32) = (10, 24);
pub(super) const TERM_FONT_FAMILY_MAX_CHARS: usize = 256;
pub(super) const TERM_FONT_FAMILY_DEFAULTS: TerminalFontFamilyDefaults =
    TerminalFontFamilyDefaults {
        macos: "SF Mono",
        windows: "Cascadia Mono",
        linux: "DejaVu Sans Mono",
    };
pub(super) const TERM_LIGATURE_MODES: [TerminalLigatureMode; 3] = [
    TerminalLigatureMode::Auto,
    TerminalLigatureMode::On,
    TerminalLigatureMode::Off,
];
pub(super) const TERM_LIGATURE_FONT_TOKENS: [&str; 20] = [
    "fira code",
    "fira mono",
    "jetbrains mono",
    "jetbrainsmono",
    "cascadia code",
    "cascadia mono",
    "iosevka",
    "victor mono",
    "hasklig",
    "monoid",
    "operator mono",
    "dank mono",
    "mononoki",
    "pragmatapro",
    "recursive",
    "monolisa",
    "commit mono",
    "geist mono",
    "maple mono",
    "departure mono",
];
pub(super) const TERM_THEME_DARK: &str = "Ghostty Default Style Dark";
pub(super) const TERM_THEME_LIGHT: &str = "Builtin Tango Light";
pub(super) const TERM_COLOR_BASE_KEYS: [TerminalColorKey; 7] = [
    TerminalColorKey::Foreground,
    TerminalColorKey::Background,
    TerminalColorKey::Cursor,
    TerminalColorKey::CursorAccent,
    TerminalColorKey::SelectionBackground,
    TerminalColorKey::SelectionForeground,
    TerminalColorKey::Bold,
];
pub(super) const TERM_COLOR_NORMAL_KEYS: [TerminalColorKey; 8] = [
    TerminalColorKey::Black,
    TerminalColorKey::Red,
    TerminalColorKey::Green,
    TerminalColorKey::Yellow,
    TerminalColorKey::Blue,
    TerminalColorKey::Magenta,
    TerminalColorKey::Cyan,
    TerminalColorKey::White,
];
pub(super) const TERM_COLOR_BRIGHT_KEYS: [TerminalColorKey; 8] = [
    TerminalColorKey::BrightBlack,
    TerminalColorKey::BrightRed,
    TerminalColorKey::BrightGreen,
    TerminalColorKey::BrightYellow,
    TerminalColorKey::BrightBlue,
    TerminalColorKey::BrightMagenta,
    TerminalColorKey::BrightCyan,
    TerminalColorKey::BrightWhite,
];
// xterm's line-height option multiplies the measured font box. CSS multiplies
// the declared font size instead, whose Orca-equivalent baseline is 1.15 here.
// Keep stored CSS values compatible while exposing Orca's 1..=3 control.
pub(super) const TERM_LINE_HEIGHT_BASE_LEADING: f32 = 1.15;
pub(super) const TERM_LINE_HEIGHT_CONTROL_BOUNDS: (f32, f32) = (1.0, 3.0);
pub(super) const TERM_LINE_HEIGHT_CONTROL_STEP: f32 = 0.1;
pub(super) const TERM_LEADING_STORAGE_BOUNDS: (f32, f32) = (1.0, 3.45);
// Orca keeps a wider normalization envelope for restored/custom values than
// the sliders it presents. Preserve that distinction: a compatible file is
// not rewritten merely because its value sits beyond the visible slider, but
// every value a person can choose in Settings follows Orca's measured range.
pub(super) const TERM_SENSITIVITY_STORAGE_BOUNDS: (f32, f32) = (0.1, 10.0);
pub(super) const TERM_SENSITIVITY_CONTROL_BOUNDS: (f32, f32) = (0.5, 3.0);
pub(super) const TERM_FAST_SCROLL_STORAGE_BOUNDS: (f32, f32) = (1.0, 20.0);
pub(super) const TERM_FAST_SCROLL_CONTROL_BOUNDS: (f32, f32) = (1.0, 10.0);
pub(super) const TERM_TUI_SCROLL_BOUNDS: (u32, u32) = (1, 10);
pub(super) const TERM_SCROLLBACK_PRESETS: [usize; 4] = [5_000, 10_000, 25_000, 50_000];
pub(super) const TERM_SCROLLBACK_CUSTOM_STEP: usize = 100;
pub(super) const TERM_CURSOR_OPACITY_BOUNDS: (f32, f32) = (0.0, 1.0);
pub(super) const TERM_PADDING_BOUNDS: (u32, u32) = (0, 512);
pub(super) const TERM_INACTIVE_PANE_OPACITY_BOUNDS: (f32, f32) = (0.0, 1.0);
pub(super) const TERM_DIVIDER_COLOR_DARK: &str = "#3f3f46";
pub(super) const TERM_DIVIDER_COLOR_LIGHT: &str = "#d4d4d8";
pub(super) const TERM_DIVIDER_THICKNESS_BOUNDS: (u32, u32) = (1, 32);
pub(super) const TERM_DIVIDER_HIT_PADDING_PX: u32 = 6;
pub(super) const TERM_WEIGHT_BOUNDS: (u32, u32) = (100, 900);
pub(super) const TERM_WEIGHT_STEP: u32 = 100;
pub(super) const TERM_BOLD_WEIGHT_FLOOR: u32 = 700;
pub(super) const TERM_BOLD_WEIGHT_OFFSET: u32 = 200;
pub(super) const TERM_CURSOR_STYLE_OPTIONS: [TerminalCursorStyle; 3] = [
    TerminalCursorStyle::Block,
    TerminalCursorStyle::Bar,
    TerminalCursorStyle::Underline,
];
pub(super) const TERM_WORD_SEPARATORS_MAX_CHARS: usize = 128;
pub(super) const DEFAULT_TERM_WORD_SEPARATORS: &str = " ()[]{}',\"`";

#[derive(Clone, Copy, Serialize)]
pub(super) struct U32SettingSpec {
    pub(super) min: u32,
    pub(super) max: u32,
    pub(super) step: u32,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct UsizeSettingSpec {
    pub(super) min: usize,
    pub(super) max: usize,
    pub(super) step: usize,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct F32SettingSpec {
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) step: f32,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct TerminalFontFamilyDefaults {
    pub(super) macos: &'static str,
    pub(super) windows: &'static str,
    pub(super) linux: &'static str,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct TerminalDividerColorDefaults {
    pub(super) dark: &'static str,
    pub(super) light: &'static str,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct TerminalThemeDefaults {
    pub(super) dark: &'static str,
    pub(super) light: &'static str,
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct TerminalColorOverrideGroup {
    pub(super) id: &'static str,
    pub(super) keys: &'static [TerminalColorKey],
}

/// Renderer control bounds. The persisted value and the UI both derive their
/// allowed range from the Rust terminal contract, so custom canonical values
/// never masquerade as the nearest preset in a `<select>`.
#[derive(Clone, Copy, Serialize)]
pub(super) struct TerminalPrefsSpec {
    pub(super) font_size: U32SettingSpec,
    pub(super) font_family_defaults: TerminalFontFamilyDefaults,
    pub(super) ligature_modes: [TerminalLigatureMode; 3],
    pub(super) mac_option_as_alt_modes: [MacOptionAsAlt; 5],
    pub(super) windows_shells: [WindowsTerminalShell; 3],
    pub(super) windows_powershell_implementations: [WindowsPowerShellImplementation; 3],
    pub(super) ligature_font_tokens: [&'static str; 20],
    pub(super) theme_defaults: TerminalThemeDefaults,
    pub(super) terminal_themes: &'static BTreeMap<String, TerminalThemeColors>,
    pub(super) color_override_groups: [TerminalColorOverrideGroup; 3],
    pub(super) leading: F32SettingSpec,
    pub(super) leading_base: f32,
    pub(super) weight: U32SettingSpec,
    pub(super) bold_weight_floor: u32,
    pub(super) bold_weight_offset: u32,
    pub(super) cursor_styles: [TerminalCursorStyle; 3],
    pub(super) cursor_opacity: F32SettingSpec,
    pub(super) padding_x: U32SettingSpec,
    pub(super) padding_y: U32SettingSpec,
    pub(super) sensitivity: F32SettingSpec,
    pub(super) fast_scroll_sensitivity: F32SettingSpec,
    pub(super) tui_scroll_sensitivity: U32SettingSpec,
    pub(super) scrollback: UsizeSettingSpec,
    pub(super) scrollback_presets: [usize; 4],
    pub(super) word_separators_max_chars: usize,
    pub(super) inactive_pane_opacity: F32SettingSpec,
    pub(super) divider_color_defaults: TerminalDividerColorDefaults,
    pub(super) divider_thickness_px: U32SettingSpec,
    pub(super) divider_hit_padding_px: u32,
    pub(super) max_custom_terminal_themes: usize,
    pub(super) custom_theme_selection_prefix: &'static str,
}

impl Default for TerminalPrefsSpec {
    fn default() -> Self {
        Self {
            font_size: U32SettingSpec {
                min: TERM_FONT_SIZE_BOUNDS.0,
                max: TERM_FONT_SIZE_BOUNDS.1,
                step: 1,
            },
            font_family_defaults: TERM_FONT_FAMILY_DEFAULTS,
            ligature_modes: TERM_LIGATURE_MODES,
            mac_option_as_alt_modes: [
                MacOptionAsAlt::Auto,
                MacOptionAsAlt::Both,
                MacOptionAsAlt::Left,
                MacOptionAsAlt::Right,
                MacOptionAsAlt::Off,
            ],
            windows_shells: [
                WindowsTerminalShell::PowerShell,
                WindowsTerminalShell::CommandPrompt,
                WindowsTerminalShell::GitBash,
            ],
            windows_powershell_implementations: [
                WindowsPowerShellImplementation::Auto,
                WindowsPowerShellImplementation::WindowsPowerShell,
                WindowsPowerShellImplementation::PowerShell7,
            ],
            ligature_font_tokens: TERM_LIGATURE_FONT_TOKENS,
            theme_defaults: TerminalThemeDefaults {
                dark: TERM_THEME_DARK,
                light: TERM_THEME_LIGHT,
            },
            terminal_themes: terminal_theme_catalog(),
            color_override_groups: [
                TerminalColorOverrideGroup {
                    id: "base",
                    keys: &TERM_COLOR_BASE_KEYS,
                },
                TerminalColorOverrideGroup {
                    id: "normal",
                    keys: &TERM_COLOR_NORMAL_KEYS,
                },
                TerminalColorOverrideGroup {
                    id: "bright",
                    keys: &TERM_COLOR_BRIGHT_KEYS,
                },
            ],
            leading: F32SettingSpec {
                min: TERM_LINE_HEIGHT_CONTROL_BOUNDS.0,
                max: TERM_LINE_HEIGHT_CONTROL_BOUNDS.1,
                step: TERM_LINE_HEIGHT_CONTROL_STEP,
            },
            leading_base: TERM_LINE_HEIGHT_BASE_LEADING,
            weight: U32SettingSpec {
                min: TERM_WEIGHT_BOUNDS.0,
                max: TERM_WEIGHT_BOUNDS.1,
                step: TERM_WEIGHT_STEP,
            },
            bold_weight_floor: TERM_BOLD_WEIGHT_FLOOR,
            bold_weight_offset: TERM_BOLD_WEIGHT_OFFSET,
            cursor_styles: TERM_CURSOR_STYLE_OPTIONS,
            cursor_opacity: F32SettingSpec {
                min: TERM_CURSOR_OPACITY_BOUNDS.0,
                max: TERM_CURSOR_OPACITY_BOUNDS.1,
                step: 0.05,
            },
            padding_x: U32SettingSpec {
                min: TERM_PADDING_BOUNDS.0,
                max: TERM_PADDING_BOUNDS.1,
                step: 1,
            },
            padding_y: U32SettingSpec {
                min: TERM_PADDING_BOUNDS.0,
                max: TERM_PADDING_BOUNDS.1,
                step: 1,
            },
            sensitivity: F32SettingSpec {
                min: TERM_SENSITIVITY_CONTROL_BOUNDS.0,
                max: TERM_SENSITIVITY_CONTROL_BOUNDS.1,
                step: 0.05,
            },
            fast_scroll_sensitivity: F32SettingSpec {
                min: TERM_FAST_SCROLL_CONTROL_BOUNDS.0,
                max: TERM_FAST_SCROLL_CONTROL_BOUNDS.1,
                step: 0.5,
            },
            tui_scroll_sensitivity: U32SettingSpec {
                min: TERM_TUI_SCROLL_BOUNDS.0,
                max: TERM_TUI_SCROLL_BOUNDS.1,
                step: 1,
            },
            scrollback: UsizeSettingSpec {
                min: zerocode_pty::MIN_SCROLLBACK_LINES,
                max: zerocode_pty::MAX_SCROLLBACK_LINES,
                step: TERM_SCROLLBACK_CUSTOM_STEP,
            },
            scrollback_presets: TERM_SCROLLBACK_PRESETS,
            word_separators_max_chars: TERM_WORD_SEPARATORS_MAX_CHARS,
            inactive_pane_opacity: F32SettingSpec {
                min: TERM_INACTIVE_PANE_OPACITY_BOUNDS.0,
                max: TERM_INACTIVE_PANE_OPACITY_BOUNDS.1,
                step: 0.05,
            },
            divider_color_defaults: TerminalDividerColorDefaults {
                dark: TERM_DIVIDER_COLOR_DARK,
                light: TERM_DIVIDER_COLOR_LIGHT,
            },
            divider_thickness_px: U32SettingSpec {
                min: TERM_DIVIDER_THICKNESS_BOUNDS.0,
                max: TERM_DIVIDER_THICKNESS_BOUNDS.1,
                step: 1,
            },
            divider_hit_padding_px: TERM_DIVIDER_HIT_PADDING_PX,
            max_custom_terminal_themes: MAX_CUSTOM_TERMINAL_THEMES,
            custom_theme_selection_prefix: CUSTOM_THEME_SELECTION_PREFIX,
        }
    }
}

pub(super) fn normalized_word_separators(raw: String) -> String {
    let kept: String = raw
        .chars()
        .filter(|character| !character.is_control())
        .take(TERM_WORD_SEPARATORS_MAX_CHARS)
        .collect();
    if kept.is_empty() {
        DEFAULT_TERM_WORD_SEPARATORS.to_string()
    } else {
        kept
    }
}

pub(super) fn normalized_terminal_font_family(raw: String) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .take(TERM_FONT_FAMILY_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) fn normalized_terminal_theme(
    raw: String,
    custom_themes: &[CustomTerminalTheme],
    fallback: &str,
) -> String {
    let chosen = raw.trim();
    if terminal_theme_catalog().contains_key(chosen)
        || has_custom_theme_selection(chosen, custom_themes)
    {
        chosen.to_string()
    } else {
        fallback.to_string()
    }
}

pub(super) fn normalized_terminal_color_overrides(
    overrides: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let allowed = TERM_COLOR_BASE_KEYS
        .iter()
        .chain(TERM_COLOR_NORMAL_KEYS.iter())
        .chain(TERM_COLOR_BRIGHT_KEYS.iter())
        .map(|key| key.as_str())
        .collect::<BTreeSet<_>>();
    overrides
        .into_iter()
        .filter(|(key, _)| allowed.contains(key.as_str()))
        .filter_map(|(key, color)| normalized_optional_hex_color(&color).map(|color| (key, color)))
        .collect()
}

impl TerminalPrefs {
    pub(super) fn clamped(self) -> Self {
        let fallback = Self::default();
        let custom_themes = normalized_custom_themes(self.custom_themes);
        let theme_dark =
            normalized_terminal_theme(self.theme_dark, &custom_themes, TERM_THEME_DARK);
        let theme_light =
            normalized_terminal_theme(self.theme_light, &custom_themes, TERM_THEME_LIGHT);
        Self {
            font_size: self
                .font_size
                .clamp(TERM_FONT_SIZE_BOUNDS.0, TERM_FONT_SIZE_BOUNDS.1),
            font_family: normalized_terminal_font_family(self.font_family),
            ligatures: self.ligatures,
            mac_option_as_alt: self.mac_option_as_alt,
            jis_yen_to_backslash: self.jis_yen_to_backslash,
            allow_osc52_clipboard: self.allow_osc52_clipboard,
            windows_shell: self.windows_shell,
            windows_powershell_implementation: self.windows_powershell_implementation,
            theme_dark,
            use_separate_light_theme: self.use_separate_light_theme,
            theme_light,
            custom_themes,
            color_overrides: normalized_terminal_color_overrides(self.color_overrides),
            // NaN cannot be clamped into range — `f32::clamp` panics on it —
            // so a file saying `null`-ish nonsense falls back rather than
            // taking the window down on the way up.
            leading: if self.leading.is_finite() {
                self.leading
                    .clamp(TERM_LEADING_STORAGE_BOUNDS.0, TERM_LEADING_STORAGE_BOUNDS.1)
            } else {
                fallback.leading
            },
            weight: self
                .weight
                .clamp(TERM_WEIGHT_BOUNDS.0, TERM_WEIGHT_BOUNDS.1),
            cursor_blink: self.cursor_blink,
            cursor_style: self.cursor_style,
            cursor_opacity: if self.cursor_opacity.is_finite() {
                self.cursor_opacity
                    .clamp(TERM_CURSOR_OPACITY_BOUNDS.0, TERM_CURSOR_OPACITY_BOUNDS.1)
            } else {
                fallback.cursor_opacity
            },
            padding_x: self
                .padding_x
                .clamp(TERM_PADDING_BOUNDS.0, TERM_PADDING_BOUNDS.1),
            padding_y: self
                .padding_y
                .clamp(TERM_PADDING_BOUNDS.0, TERM_PADDING_BOUNDS.1),
            sensitivity: if self.sensitivity.is_finite() {
                self.sensitivity.clamp(
                    TERM_SENSITIVITY_STORAGE_BOUNDS.0,
                    TERM_SENSITIVITY_STORAGE_BOUNDS.1,
                )
            } else {
                fallback.sensitivity
            },
            scrollback: self.scrollback.clamp(
                zerocode_pty::MIN_SCROLLBACK_LINES,
                zerocode_pty::MAX_SCROLLBACK_LINES,
            ),
            word_separators: normalized_word_separators(self.word_separators),
            fast_scroll_sensitivity: if self.fast_scroll_sensitivity.is_finite() {
                self.fast_scroll_sensitivity.clamp(
                    TERM_FAST_SCROLL_STORAGE_BOUNDS.0,
                    TERM_FAST_SCROLL_STORAGE_BOUNDS.1,
                )
            } else {
                fallback.fast_scroll_sensitivity
            },
            tui_scroll_sensitivity: self
                .tui_scroll_sensitivity
                .clamp(TERM_TUI_SCROLL_BOUNDS.0, TERM_TUI_SCROLL_BOUNDS.1),
            focus_follows_mouse: self.focus_follows_mouse,
            hide_mouse_while_typing: self.hide_mouse_while_typing,
            copy_on_select: self.copy_on_select,
            right_click_paste: self.right_click_paste,
            inactive_pane_opacity: if self.inactive_pane_opacity.is_finite() {
                self.inactive_pane_opacity.clamp(
                    TERM_INACTIVE_PANE_OPACITY_BOUNDS.0,
                    TERM_INACTIVE_PANE_OPACITY_BOUNDS.1,
                )
            } else {
                fallback.inactive_pane_opacity
            },
            divider_color_dark: normalized_hex_color(
                &self.divider_color_dark,
                TERM_DIVIDER_COLOR_DARK,
            ),
            divider_color_light: normalized_hex_color(
                &self.divider_color_light,
                TERM_DIVIDER_COLOR_LIGHT,
            ),
            divider_thickness_px: self.divider_thickness_px.clamp(
                TERM_DIVIDER_THICKNESS_BOUNDS.0,
                TERM_DIVIDER_THICKNESS_BOUNDS.1,
            ),
        }
    }
}

pub(super) fn stored_terminal_prefs(legacy: &LegacySettings<'_>) -> TerminalPrefs {
    legacy
        .read::<TerminalPrefs>(legacy_settings_file::TERMINAL_PREFS)
        .unwrap_or_default()
        .clamped()
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct WindowsTerminalStatus {
    pub(super) supported: bool,
    pub(super) pwsh_available: bool,
    pub(super) git_bash_available: bool,
}

/// The language the person chose, or `system` when they have not chosen.
pub(super) fn stored_locale(legacy: &LegacySettings<'_>) -> String {
    let kept: String = legacy
        .read(legacy_settings_file::LOCALE)
        .unwrap_or_default();
    if LOCALES.contains(&kept.as_str()) {
        kept
    } else {
        "system".to_string()
    }
}
