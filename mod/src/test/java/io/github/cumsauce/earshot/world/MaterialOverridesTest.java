package io.github.cumsauce.earshot.world;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The hand-written overrides in {@code config/earshot/materials.json}: an object from block ids or {@code #tag} keys
 * to material names. Read by size — {@link MaterialOverrides#find} needs a registered block state, which belongs to a
 * running game, while the parsing itself (what is kept, what is skipped, what is refused outright) does not.
 */
class MaterialOverridesTest {
	@TempDir
	Path dir;

	/** No materials.json means Earshot's own guesses stand, with nothing set by hand. */
	@Test
	void noFileMeansNoOverrides() {
		assertEquals(0, MaterialOverrides.load(dir.resolve("absent.json")).size());
	}

	/** A block id and a #tag are taken; an unknown material name, a non-name value and a bad key are skipped. */
	@Test
	void blockIdsAndTagsAreTakenAndBadEntriesSkipped() throws IOException {
		Path file = dir.resolve("materials.json");
		Files.writeString(file, """
				{
					"minecraft:hay_block": "cloth",
					"#minecraft:logs": "wood",
					"minecraft:stone": "not_a_material",
					"minecraft:dirt": 3,
					"not an identifier": "rock"
				}
				""");
		assertEquals(2, MaterialOverrides.load(file).size());
	}

	/** A file that is not JSON, or JSON that is not an object, sets nothing at all. */
	@Test
	void entriesThatAreNotAnObjectOrNotJsonAreIgnored() throws IOException {
		Path array = dir.resolve("array.json");
		Files.writeString(array, "[\"minecraft:hay_block\"]");
		assertEquals(0, MaterialOverrides.load(array).size());
		Path broken = dir.resolve("broken.json");
		Files.writeString(broken, "{ minecraft:hay_block");
		assertEquals(0, MaterialOverrides.load(broken).size());
	}

	/** An object holding nothing valid is an empty set of overrides, not a failure. */
	@Test
	void anObjectOfNothingValidIsStillAnEmptySetOfOverrides() throws IOException {
		Path file = dir.resolve("all-bad.json");
		Files.writeString(file, """
				{
					"minecraft:stone": "not_a_material",
					"#bad key": "wood",
					"minecraft:dirt": null
				}
				""");
		assertEquals(0, MaterialOverrides.load(file).size());
	}
}
