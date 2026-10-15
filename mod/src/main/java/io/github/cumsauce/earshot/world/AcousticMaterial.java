package io.github.cumsauce.earshot.world;

import java.util.Locale;

/**
 * What a block is made of, acoustically: the native {@code Material} ids, in the same order. The native table
 * ({@code crates/earshot-world/src/material.rs}) holds the absorption, scattering and transmission of each.
 */
public enum AcousticMaterial {
	/** Also anything that doesn't stop sound: plants, open doors, fences, small blocks. */
	AIR,
	ROCK,
	WOOD,
	GLASS,
	/** Wool, carpet, beds, slime, sponge: soft and absorbent. */
	CLOTH,
	LEAVES,
	SOIL,
	SAND,
	METAL,
	WATER,
	LAVA,
	ICE,
	SNOW,
	/** A thin soft layer that doesn't stop sound but soaks it up: carpets, snow layers. */
	RUG;

	/** The id the native world stores. */
	public byte id() {
		return (byte) ordinal();
	}

	/** The name the native table uses. */
	public String nativeName() {
		return name().toLowerCase(Locale.ROOT);
	}
}
