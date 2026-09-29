package p;
public enum ConstKind {
    A(Numbers.ONE), B(Numbers.TWO);
    private final int code;
    ConstKind(int code) { this.code = code; }
    public int getCode() { return code; }
}
class Numbers {
    static final int ONE = 1;
    static final int TWO = 2;
}
