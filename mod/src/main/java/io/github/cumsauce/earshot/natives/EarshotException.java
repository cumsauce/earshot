package io.github.cumsauce.earshot.natives;

/** A native call failed. {@code code} is one of the {@code EARSHOT_ERR_*} values in {@code earshot.h}. */
public final class EarshotException extends RuntimeException {
	public static final int INVALID_ARGUMENT = -1;
	public static final int ABI_MISMATCH = -2;
	public static final int PANIC = -3;

	private final int code;

	public EarshotException(int code, String message) {
		super(message + " (error " + code + ")");
		this.code = code;
	}

	public int code() {
		return code;
	}
}
