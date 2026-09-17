# hermetic env for zo, mirroring zo-ide/crates/zo-ide/tests/e2e/harness.rs configure_command
R=${PARITY_ROOT:-/tmp/zo-agent-parity-20260907}/sandbox
export HOME=$R/home USERPROFILE=$R/home ZO_CONFIG_HOME=$R/home TMPDIR=$R/state
export ZO_DISABLE_MODEL_DISCOVERY=1
export CODEX_HOME=$R/home/codex ZO_CODEX_HOME=$R/home/codex
export CLAUDE_CONFIG_DIR=$R/home/claude
export ZO_SESSION_ROOT=$R/sessions ZO_STATE_DIR=$R/state
export ANTHROPIC_API_KEY=test-dummy-key
export ZO_DISABLE_KEYCHAIN=1 ZO_DISABLE_EXTERNAL_CREDENTIALS=1
export TERM=xterm-256color RUST_BACKTRACE=1
unset NO_COLOR ZO_HOME ANTHROPIC_AUTH_TOKEN OPENAI_API_KEY XAI_API_KEY GOOGLE_API_KEY GEMINI_API_KEY ZO_CUSTOM_PROVIDERS
unset ZEROCODE_SECOND_BRAIN ZO_AUTO_VERIFY ZO_AUTO_VERIFY_CMD ZO_TUI_FOLD_MARKERS
unset ZEROCODE_HOOK_PORT ZEROCODE_HOOK_TOKEN ZEROCODE_PANE_KEY
for v in $(env | LC_ALL=C grep -o '^ZEROCODE_[A-Z_]*' ; env | LC_ALL=C grep -o '^ZO_EVENTS_[A-Z_]*'; env | LC_ALL=C grep -o '^ZO_SERVE_[A-Z_]*'; env | LC_ALL=C grep -o '^ZO_MODEL_[A-Z_]*'; env | LC_ALL=C grep -o '^ZO_LAUNCH_[A-Z_]*'); do unset $v; done
