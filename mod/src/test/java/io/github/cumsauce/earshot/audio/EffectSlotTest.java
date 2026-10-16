package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

/**
 * The EFX pairing {@link SoundShaper#makeEffectSlot} keeps: an effect and the slot that plays it are made together,
 * and a pair OpenAL would only half hand out is let go at once, each name by its own. Needs no OpenAL context.
 */
class EffectSlotTest {
	/** A device out of slots gives 0 for the slot: the effect it would have played must not be left live. */
	@Test
	void a_slot_that_will_not_generate_does_not_leak_its_effect() {
		List<Integer> deletedEffects = new ArrayList<>();
		List<Integer> deletedSlots = new ArrayList<>();

		SoundShaper.EffectSlot made = SoundShaper.makeEffectSlot(
				() -> 5, () -> 0, i -> deletedEffects.add(i), i -> deletedSlots.add(i));

		assertEquals(0, made.effect());
		assertEquals(0, made.slot(), "a slot OpenAL never gave is not a slot to send to");
		assertEquals(List.of(5), deletedEffects, "the effect made for a slot that never came must be deleted");
		assertEquals(List.of(), deletedSlots);
	}

	/** An effect OpenAL would not make leaves a slot with nothing to play: the slot must go too. */
	@Test
	void an_effect_that_will_not_generate_does_not_leak_its_slot() {
		List<Integer> deletedEffects = new ArrayList<>();
		List<Integer> deletedSlots = new ArrayList<>();

		SoundShaper.EffectSlot made = SoundShaper.makeEffectSlot(
				() -> 0, () -> 7, i -> deletedEffects.add(i), i -> deletedSlots.add(i));

		assertEquals(0, made.effect());
		assertEquals(0, made.slot());
		assertEquals(List.of(), deletedEffects);
		assertEquals(List.of(7), deletedSlots, "a slot with no effect must be given back");
	}

	/** Both names came: the pair is kept whole and nothing is deleted. */
	@Test
	void a_pair_that_generates_is_kept_whole() {
		List<Integer> deletedEffects = new ArrayList<>();
		List<Integer> deletedSlots = new ArrayList<>();

		SoundShaper.EffectSlot made = SoundShaper.makeEffectSlot(
				() -> 5, () -> 7, i -> deletedEffects.add(i), i -> deletedSlots.add(i));

		assertEquals(5, made.effect());
		assertEquals(7, made.slot());
		assertEquals(List.of(), deletedEffects);
		assertEquals(List.of(), deletedSlots);
	}

	/** Teardown deletes both names of a whole pair. */
	@Test
	void teardown_deletes_both_names_of_a_whole_pair() {
		List<Integer> deletedEffects = new ArrayList<>();
		List<Integer> deletedSlots = new ArrayList<>();

		SoundShaper.deleteEffectSlot(5, 7, i -> deletedEffects.add(i), i -> deletedSlots.add(i));

		assertEquals(List.of(7), deletedSlots);
		assertEquals(List.of(5), deletedEffects);
	}

	/** An effect whose slot was refused is still deleted, by its own name alone. */
	@Test
	void teardown_deletes_an_effect_whose_slot_is_already_gone() {
		List<Integer> deletedEffects = new ArrayList<>();
		List<Integer> deletedSlots = new ArrayList<>();

		SoundShaper.deleteEffectSlot(5, 0, i -> deletedEffects.add(i), i -> deletedSlots.add(i));

		assertEquals(List.of(), deletedSlots);
		assertEquals(List.of(5), deletedEffects, "the effect must not be skipped because the slot is 0");
	}
}
