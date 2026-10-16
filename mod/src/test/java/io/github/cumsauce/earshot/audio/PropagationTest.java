package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import net.minecraft.client.resources.sounds.Sound;
import net.minecraft.client.resources.sounds.SoundInstance;
import net.minecraft.client.sounds.SoundManager;
import net.minecraft.client.sounds.WeighedSoundEvents;
import net.minecraft.resources.Identifier;
import net.minecraft.sounds.SoundSource;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/** Earshot's held-sound queue, exercised without the game: hold, let out, cancel, forget. */
class PropagationTest {
	/**
	 * A sound the game could play. The queue only carries the reference — nothing here is ever called — so the
	 * members stand empty and {@link #getX()} tells the stubs apart.
	 */
	private static final class StubSound implements SoundInstance {
		private final double at;

		StubSound(double at) {
			this.at = at;
		}

		@Override
		public Identifier getIdentifier() {
			return null;
		}

		@Override
		public WeighedSoundEvents getOrResolve(SoundManager manager) {
			return null;
		}

		@Override
		public Sound getSound() {
			return null;
		}

		@Override
		public WeighedSoundEvents getSoundEvent() {
			return null;
		}

		@Override
		public SoundSource getSource() {
			return SoundSource.MASTER;
		}

		@Override
		public boolean isLooping() {
			return false;
		}

		@Override
		public boolean isRelative() {
			return false;
		}

		@Override
		public int getDelay() {
			return 0;
		}

		@Override
		public float getVolume() {
			return 1;
		}

		@Override
		public float getPitch() {
			return 1;
		}

		@Override
		public double getX() {
			return at;
		}

		@Override
		public double getY() {
			return 64;
		}

		@Override
		public double getZ() {
			return at;
		}

		@Override
		public Attenuation getAttenuation() {
			return Attenuation.LINEAR;
		}
	}

	/** The queue is static, so every test starts and ends with it empty. */
	@BeforeEach
	@AfterEach
	void emptyTheQueue() {
		Propagation.clear();
	}

	/** Held one tick, a sound comes out on the next letting-out — and plays but the once. */
	@Test
	void aSoundHeldOneTickComesOutOnTheNextTick() {
		SoundInstance sound = new StubSound(0);
		Propagation.hold(sound, 1);

		List<SoundInstance> due = Propagation.due();
		assertEquals(1, due.size());
		assertEquals(sound, due.get(0));
		assertTrue(Propagation.due().isEmpty(), "a sound plays once, not round again");
	}

	/** Held two ticks, the sound is still waiting on the first tick's letting-out. */
	@Test
	void aSoundHeldTwoTicksWaitsThroughTheFirstTick() {
		SoundInstance sound = new StubSound(0);
		Propagation.hold(sound, 2);

		assertTrue(Propagation.due().isEmpty(), "still waiting its second tick");
		assertEquals(List.of(sound), Propagation.due(), "due the tick after");
	}

	/** A sound stopped before it started is forgotten: it never comes out. */
	@Test
	void aCancelledSoundNeverComesOut() {
		SoundInstance sound = new StubSound(0);
		Propagation.hold(sound, 1);
		Propagation.cancel(sound);

		assertTrue(Propagation.due().isEmpty(), "cancelled before it was due");
	}

	/** Clearing forgets every held sound, so the same one can be held afresh. */
	@Test
	void clearingTheQueueLetsAHeldSoundBeHeldAgain() {
		SoundInstance sound = new StubSound(0);
		Propagation.hold(sound, 5);
		Propagation.clear();

		Propagation.hold(sound, 1);
		assertEquals(List.of(sound), Propagation.due(), "held once after the clearing, once due");
	}
}
