//! Enabling dct's own `[llm]` feature after a pairing hands over a key.
//!
//! **Task 5's job, not this one's.** `pair_apply::apply` decides *whether* and
//! *with which provider/model* to opt in, then calls this function to do the
//! actual write. Until Task 5 lands, this is a stub that always declines —
//! wiring the call site now (Task 4) and leaving the effect until Task 5
//! is the whole point of splitting the two: a checkbox that silently does
//! nothing is worse than one that visibly hasn't shipped yet.
pub fn enable(config_path: &std::path::Path, provider: &str, model: &str) -> Result<bool, String> {
    let _ = (config_path, provider, model);
    Ok(false)
}
