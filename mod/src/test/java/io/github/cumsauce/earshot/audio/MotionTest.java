package io.github.cumsauce.earshot.audio;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import net.minecraft.world.phys.Vec3;
import org.junit.jupiter.api.Test;

class MotionTest {
	private static final long TICK = 50_000_000L;

	@Test
	void aSteadyMoverIsTrackedAndAJumpIsNotAVelocity() {
		SourceState.Motion motion = new SourceState.Motion();
		Vec3 velocity = Vec3.ZERO;
		// 10 blocks a second along x, a tick at a time.
		for (int tick = 0; tick < 20; tick++) velocity = motion.update(new Vec3(0.5 * tick, 0, 0), tick * TICK);
		assertEquals(10, velocity.x, 0.01);
		assertEquals(0, velocity.y, 1e-9);
		// A teleport 100 blocks away is a jump, not 2000 blocks a second.
		velocity = motion.update(new Vec3(110, 0, 0), 20 * TICK);
		assertEquals(Vec3.ZERO, velocity);
		// Standing still settles to nothing.
		for (int tick = 21; tick < 40; tick++) velocity = motion.update(new Vec3(110, 0, 0), tick * TICK);
		assertTrue(velocity.length() < 1e-3, velocity.toString());
	}
}
