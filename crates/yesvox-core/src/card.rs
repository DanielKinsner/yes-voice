//! The Standard-view **card model** (UI ref `docs/ui/yesvox-v1-ui-reference.md`).
//!
//! The reference shell groups the rack into ten cards the *user* sees, and they
//! are deliberately **not** 1:1 with the engine [`Stage`]s: one card can drive two
//! stages (De-click / De-pop → `DeClick` + `DePop`) and three cards share one
//! stage (Presence/Air, Leveling, Loudness all ride `FinishStage`). Conversely
//! `DeClip`/`DeBleed` are automatic restoration with no card at all.
//!
//! This module is that presentation layer: the stable, agent-callable card
//! taxonomy the egui shell — and any agent — addresses, mapped onto the engine
//! stages underneath. The overlapping sub-controls (the mouth-click path inside
//! `DeClick`, the EQ/leveler/loudness sub-modules inside `FinishStage`) are
//! refined into independent knobs by their own [`crate::Session`] surfaces; this
//! enum is where they get a name.

use crate::chain::Stage;

/// A Standard-view rack card — five **Restore** then five **Polish**, in display
/// order (UI ref §RESTORE/POLISH racks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardId {
    // --- Restore rack ---
    /// Reduces background noise while preserving detail.
    Denoise,
    /// Removes reverb while preserving natural space.
    DeReverb,
    /// Removes clicks, pops and crackle — drives both transient stages.
    DeClickDePop,
    /// Reduces lip smacks and mouth noises — the `MouthClick` sub-path of `DeClick`.
    MouthClicks,
    /// Removes electrical hum and low-frequency noise.
    DeHum,
    // --- Polish rack ---
    /// Controls harsh sibilance without dulling.
    DeEss,
    /// Attenuates harsh breaths (voiceover-leaning).
    Breath,
    /// Adds clarity and air — the finish EQ.
    PresenceAir,
    /// Smooths vocal level consistently — the finish program leveler.
    Leveling,
    /// Sets loudness to a target level — the finish loudness target + limiter.
    Loudness,
}

impl CardId {
    /// Every card, Restore rack first then Polish, in display order.
    pub const ALL: [CardId; 10] = [
        CardId::Denoise,
        CardId::DeReverb,
        CardId::DeClickDePop,
        CardId::MouthClicks,
        CardId::DeHum,
        CardId::DeEss,
        CardId::Breath,
        CardId::PresenceAir,
        CardId::Leveling,
        CardId::Loudness,
    ];

    /// The Restore-rack cards (top group).
    pub const RESTORE: [CardId; 5] = [
        CardId::Denoise,
        CardId::DeReverb,
        CardId::DeClickDePop,
        CardId::MouthClicks,
        CardId::DeHum,
    ];

    /// The Polish-rack cards (bottom group).
    pub const POLISH: [CardId; 5] = [
        CardId::DeEss,
        CardId::Breath,
        CardId::PresenceAir,
        CardId::Leveling,
        CardId::Loudness,
    ];

    /// The engine [`Stage`]s this card's enable toggle governs. Composite cards
    /// list every stage they touch; the overlapping mouth-click / finish
    /// sub-controls are refined by their dedicated `Session` surfaces.
    pub fn stages(self) -> &'static [Stage] {
        match self {
            CardId::Denoise => &[Stage::Denoise],
            CardId::DeReverb => &[Stage::DeReverb],
            CardId::DeClickDePop => &[Stage::DeClick, Stage::DePop],
            CardId::MouthClicks => &[Stage::DeClick],
            CardId::DeHum => &[Stage::DeHum],
            CardId::DeEss => &[Stage::DeEss],
            CardId::Breath => &[Stage::Breath],
            CardId::PresenceAir => &[Stage::Finish],
            CardId::Leveling => &[Stage::Finish],
            CardId::Loudness => &[Stage::Finish],
        }
    }

    /// The card's display title (matches the UI-reference card labels).
    pub fn label(self) -> &'static str {
        match self {
            CardId::Denoise => "Denoise",
            CardId::DeReverb => "De-reverb",
            CardId::DeClickDePop => "De-click / De-pop",
            CardId::MouthClicks => "Mouth Clicks",
            CardId::DeHum => "De-hum",
            CardId::DeEss => "De-ess",
            CardId::Breath => "Breath Control",
            CardId::PresenceAir => "Presence / Air",
            CardId::Leveling => "Leveling",
            CardId::Loudness => "Loudness",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn racks_partition_all_cards_in_order() {
        let mut both: Vec<CardId> = CardId::RESTORE.to_vec();
        both.extend_from_slice(&CardId::POLISH);
        assert_eq!(both, CardId::ALL.to_vec(), "RESTORE++POLISH == ALL");
    }

    #[test]
    fn composite_cards_map_to_multiple_or_shared_stages() {
        assert_eq!(
            CardId::DeClickDePop.stages(),
            &[Stage::DeClick, Stage::DePop]
        );
        // The three finish cards all ride the one Finish stage.
        for c in [CardId::PresenceAir, CardId::Leveling, CardId::Loudness] {
            assert_eq!(c.stages(), &[Stage::Finish]);
        }
    }
}
