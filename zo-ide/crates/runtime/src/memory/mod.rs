pub mod classification;
pub mod curation;
pub mod dreamer;
#[cfg(feature = "memory-embed")]
pub mod embed_fastembed;
pub mod model_tag;
pub mod paths;
pub mod recall;
pub mod recall_seat;
pub mod rerank;

pub use classification::{
    MemoryClassification, MemoryKind, MemoryScope, MemorySource, classify_memory_body,
    memory_body_has_classification_metadata, with_memory_scope_metadata,
};
pub use model_tag::MemoryModelTag;

pub use dreamer::{
    dream_at_cwd, evaluate_manual_apply_gate, latest_dream_fusion_report,
    mark_self_improve_candidate_applied, mark_self_improve_candidate_rejected,
    maybe_auto_dream, maybe_auto_dream_on_compaction, read_self_improve_candidates, read_self_improve_schedule_state,
    read_self_improve_schedule_state_readonly, record_auto_dream_failure, record_automation_event,
    record_observation, record_self_improve_attempt, record_self_improve_candidate,
    record_self_improve_failure, record_self_improve_pulse, record_self_improve_pulse_if_enabled,
    record_user_pattern_observation, record_verified_check,
    run_dream_fusion_v0, run_quarantine_patch, should_auto_dream, should_run_self_improve,
    spawn_auto_dream_on_compaction,
    trusted_git_binary, write_hand_written_memory_entry,
    try_acquire_self_improve_lock, verified_check_observation, write_dream_fusion_report,
    AppliedPromotion, AutomationLessonSource, CompositeLessonSource, DreamError, DreamReport,
    Dreamer, FsMemoryStore, JsonlLessonSource, LessonSource, ManualApplyGateRequest,
    MemoryStore, MemoryWriteRequest, QuarantineCheckCommand, QuarantinePatchRequest,
    SelfImproveLock, TurnLogLessonSource, UserPatternLessonSource, WriteOutcome,
    DEFAULT_AUTO_DREAM_INTERVAL, DEFAULT_COMPACTION_DREAM_INTERVAL,
};
pub use recall::{
    load_lexical_memory_retriever, load_memory_retriever, parse_memory_index,
    render_recalled_memory_section, IndexedCorpusPage, LexicalMemoryRetriever, RecallDemand,
    RecallDemandSource,
};
