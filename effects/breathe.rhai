// The simplest useful script: one colour, gently pulsing.
// A good starting point to copy.

fn meta() {
    #{
        name: "Breathe",
        description: "A single colour fading in and out",
        params: [
            #{ id: "color",  label: "Colour", kind: "color", "default": "#ff2288" },
            #{ id: "period", label: "Period", kind: "float", min: 0.5, max: 10.0, "default": 4.0 },
        ],
    }
}

fn render(t, keys, p) {
    // 0 -> 1 -> 0 over `period` seconds
    let phase = frac(t / p.period);
    let v = 0.5 - 0.5 * cos(phase * 2.0 * PI());

    let out = [];
    for k in keys {
        out.push([p.color.r * v, p.color.g * v, p.color.b * v]);
    }
    out
}
