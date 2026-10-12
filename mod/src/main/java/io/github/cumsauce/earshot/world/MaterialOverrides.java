package io.github.cumsauce.earshot.world;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import io.github.cumsauce.earshot.EarshotClient;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import org.jspecify.annotations.Nullable;

/**
 * Materials set by hand in {@code config/earshot/materials.json}, over Earshot's own guesses: an object from block ids
 * ({@code "minecraft:hay_block"}) or block tags ({@code "#minecraft:logs"}) to material names ({@code "cloth"}). Block
 * ids win over tags, and earlier tags over later ones.
 */
public final class MaterialOverrides {
	public static final MaterialOverrides NONE = new MaterialOverrides(Map.of(), List.of());

	private final Map<Identifier, AcousticMaterial> blocks;
	private final List<Map.Entry<TagKey<Block>, AcousticMaterial>> tags;

	private MaterialOverrides(Map<Identifier, AcousticMaterial> blocks,
			List<Map.Entry<TagKey<Block>, AcousticMaterial>> tags) {
		this.blocks = blocks;
		this.tags = tags;
	}

	/** How many entries there are. */
	public int size() {
		return blocks.size() + tags.size();
	}

	/** The material set for {@code state}, or null if none is. */
	public @Nullable AcousticMaterial find(BlockState state) {
		AcousticMaterial byBlock = blocks.get(BuiltInRegistries.BLOCK.getKey(state.getBlock()));
		if (byBlock != null) return byBlock;
		for (Map.Entry<TagKey<Block>, AcousticMaterial> tag : tags) {
			if (state.is(tag.getKey())) return tag.getValue();
		}
		return null;
	}

	/** The overrides in {@code file}; none if there is no file. Entries that don't parse are logged and skipped. */
	public static MaterialOverrides load(Path file) {
		JsonElement json;
		try {
			json = JsonParser.parseString(Files.readString(file));
		} catch (NoSuchFileException e) {
			return NONE;
		} catch (IOException | JsonParseException e) {
			EarshotClient.LOGGER.warn("Couldn't read material overrides from {}", file, e);
			return NONE;
		}
		if (!(json instanceof JsonObject object)) {
			EarshotClient.LOGGER.warn("{} should hold an object from block ids or #tags to materials", file);
			return NONE;
		}
		Map<Identifier, AcousticMaterial> blocks = new HashMap<>();
		List<Map.Entry<TagKey<Block>, AcousticMaterial>> tags = new ArrayList<>();
		for (Map.Entry<String, JsonElement> entry : object.entrySet()) {
			String key = entry.getKey();
			AcousticMaterial material = entry.getValue() instanceof JsonPrimitive p && p.isString()
					? material(p.getAsString())
					: null;
			Identifier id = Identifier.tryParse(key.startsWith("#") ? key.substring(1) : key);
			if (material == null || id == null) {
				EarshotClient.LOGGER.warn("Skipped material override {}: {} (materials are {})", key, entry.getValue(),
						names());
				continue;
			}
			if (key.startsWith("#")) {
				tags.add(Map.entry(TagKey.create(Registries.BLOCK, id), material));
			} else {
				blocks.put(id, material);
			}
		}
		return new MaterialOverrides(Map.copyOf(blocks), List.copyOf(tags));
	}

	private static @Nullable AcousticMaterial material(String name) {
		for (AcousticMaterial material : AcousticMaterial.values()) {
			if (material.name().equalsIgnoreCase(name.trim())) return material;
		}
		return null;
	}

	private static String names() {
		List<String> names = new ArrayList<>();
		for (AcousticMaterial material : AcousticMaterial.values()) names.add(material.name().toLowerCase(Locale.ROOT));
		return String.join(", ", names);
	}
}
