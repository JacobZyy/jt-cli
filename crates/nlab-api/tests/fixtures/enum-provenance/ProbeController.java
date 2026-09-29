package p;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.bind.annotation.GetMapping;
@RestController
public class ProbeController {
    @GetMapping("/getter")
    public Payload getter() {
        Payload v = new Payload();
        v.setCode(Kind.A.getCode());
        return v;
    }
    @GetMapping("/name")
    public Payload name() {
        Payload v = new Payload();
        v.setToken(Kind.A.name());
        return v;
    }
    @GetMapping("/val")
    public Payload val() {
        Payload v = new Payload();
        v.setCode(Kind.A.val());
        return v;
    }
    @GetMapping("/codeMethod")
    public Payload codeMethod() {
        Payload v = new Payload();
        v.setCode(Kind.A.code());
        return v;
    }
    @GetMapping("/directEnumField")
    public Payload directEnumField() {
        Payload v = new Payload();
        v.setCode(Kind.A.code);
        return v;
    }
    @GetMapping("/helper")
    public Payload helper() {
        Payload v = new Payload();
        v.setCode(project(Kind.A));
        return v;
    }
    @GetMapping("/literalExtra")
    public Payload literalExtra() {
        Payload v = new Payload();
        if (System.currentTimeMillis() > 0) { v.setCode(Kind.A.getCode()); } else { v.setCode(99); }
        return v;
    }
    @GetMapping("/optional")
    public Payload optional() {
        Payload v = new Payload();
        v.setCode(java.util.Optional.of(Kind.A).map(Kind::getCode).orElse(99));
        return v;
    }
    @GetMapping("/methodReference")
    public Payload methodReference() {
        Payload v = new Payload();
        java.util.Optional.of(Kind.A).map(Kind::getCode).ifPresent(v::setCode);
        return v;
    }
    @GetMapping("/directWrite")
    public Payload directWrite() {
        Payload v = new Payload();
        v.code = Kind.A.getCode();
        return v;
    }
    @GetMapping("/constructor")
    public Payload constructor() {
        Payload v = new Payload();
        return new Payload(Kind.A.getCode());
    }
    @GetMapping("/builder")
    public Payload builder() {
        Payload v = new Payload();
        return Payload.builder().code(Kind.A.getCode()).build();
    }
    @GetMapping("/collection")
    public Payload collection() {
        Payload v = new Payload();
        v.setCodes(java.util.Arrays.stream(Kind.values()).map(Kind::getCode).collect(java.util.stream.Collectors.toList()));
        return v;
    }
    @GetMapping("/enumField")
    public Payload enumField() {
        Payload v = new Payload();
        v.setKind(Kind.A);
        return v;
    }
    @GetMapping("/loopLookup")
    public Payload loopLookup() {
        Payload v = new Payload();
        return loopLookupFrom(new Source());
    }
    @GetMapping("/mapLookup")
    public Payload mapLookup() {
        Payload v = new Payload();
        return mapLookupFrom(new Source());
    }
    private Integer project(Kind kind) { return kind.getCode(); }
    private Payload loopLookupFrom(Source s) {
        Kind from = Kind.fromLoop(s.getCode());
        Payload v = new Payload();
        v.setCode(s.getCode());
        return v;
    }
    private Payload mapLookupFrom(Source s) {
        Kind from = Kind.fromMap(s.getCode());
        Payload v = new Payload();
        v.setCode(s.getCode());
        return v;
    }
    @GetMapping("/enumConstantArgument")
    public Payload enumConstantArgument() {
        Payload v = new Payload();
        v.setCode(ConstKind.A.getCode());
        return v;
    }
    @GetMapping("/fieldInitializer")
    public DefaultPayload fieldInitializer(@org.springframework.web.bind.annotation.RequestParam boolean replace) {
        DefaultPayload v = new DefaultPayload();
        if (replace) { v.setCode(Kind.A.getCode()); }
        return v;
    }
}
