package p;
public enum Kind {
    A(1), B(2);
    public final int code;
    Kind(int code) { this.code = code; }
    public int getCode() { return code; }
    public int val() { return code; }
    public int code() { return code; }
    private static final java.util.Map<Integer, Kind> INDEX = java.util.Arrays.stream(values()).collect(java.util.stream.Collectors.toMap(Kind::getCode, x -> x));
    public static Kind fromMap(Integer input) { return INDEX.get(input); }
    public static Kind fromLoop(Integer input) { for (Kind item : values()) { if (java.util.Objects.equals(item.getCode(), input)) { return item; } } return null; }
}
