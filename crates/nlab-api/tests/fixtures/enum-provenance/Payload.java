package p;
public class Payload {
    public Integer code;
    private String token;
    private java.util.List<Integer> codes;
    private Kind kind;
    public Payload() {}
    public Payload(Integer code) { this.code = code; }
    public void setCode(Integer code) { this.code = code; }
    public void setToken(String token) { this.token = token; }
    public void setCodes(java.util.List<Integer> codes) { this.codes = codes; }
    public void setKind(Kind kind) { this.kind = kind; }
    public static Builder builder() { return new Builder(); }
    public static class Builder {
        private Integer code;
        public Builder code(Integer code) { this.code = code; return this; }
        public Payload build() { return new Payload(code); }
    }
}
