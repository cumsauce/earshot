package io.github.cumsauce.earshot.world;

import static org.junit.jupiter.api.Assertions.assertEquals;

import io.github.cumsauce.earshot.natives.NativeBridge;
import io.github.cumsauce.earshot.natives.NativeLoader;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class AcousticMaterialTest {
	@Test
	void namesAndOrderMatchTheNativeTable(@TempDir Path configDir) throws Exception {
		NativeBridge bridge = new NativeBridge(NativeLoader.load(configDir));
		String json = bridge.materialNames();
		List<String> nativeNames = Arrays.stream(json.substring(1, json.length() - 1).split(","))
				.map(name -> name.substring(1, name.length() - 1))
				.toList();
		List<String> ours = Arrays.stream(AcousticMaterial.values()).map(AcousticMaterial::nativeName).toList();
		assertEquals(nativeNames, ours);
	}
}
