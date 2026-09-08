// Expanding rings from the middle of the board.

fn meta() {
    #{
        name: "Ripple",
        description: "Rings spreading out from the centre",
        params: [
            #{ id: "speed",  label: "Speed",     kind: "float", min: 0.1, max: 2.0, "default": 0.5 },
            #{ id: "spacing", label: "Spacing",  kind: "float", min: 0.1, max: 1.0, "default": 0.30 },
            #{ id: "hue",    label: "Hue",       kind: "float", min: 0.0, max: 1.0, "default": 0.55 },
        ],
    }
}

fn render(t, keys, p) {
    let out = [];
    for k in keys {
        let d = dist(k.nx, k.ny, 0.5, 0.5);
        // rings move outward as t grows
        let v = 0.5 + 0.5 * cos((d / p.spacing - t * p.speed) * 2.0 * PI());
        out.push(hsv(p.hue + d * 0.2, 0.9, v * v));
    }
    out
}
