// A rotating arm sweeping around the centre of the board.
//
// Drop this file in `effects/` and the app picks it up — edit and save while
// it is running and the change applies live.

fn meta() {
    #{
        name: "Spiral",
        description: "A rotating arm sweeping around the board",
        params: [
            #{ id: "speed", label: "Speed",      kind: "float", min: 0.0, max: 3.0, "default": 0.6 },
            #{ id: "arms",  label: "Arms",       kind: "int",   min: 1,   max: 6,   "default": 2 },
            #{ id: "trail", label: "Trail",      kind: "float", min: 0.05, max: 1.0, "default": 0.35 },
            #{ id: "bright", label: "Brightness", kind: "float", min: 0.0, max: 1.0, "default": 1.0 },
        ],
    }
}

fn render(t, keys, p) {
    let out = [];
    for k in keys {
        // centre the coordinates, -0.5 .. 0.5
        let dx = k.nx - 0.5;
        let dy = k.ny - 0.5;

        // angle of this key, normalised to 0..1
        let ang = atan(dy, dx) / (2.0 * PI());
        if ang < 0.0 { ang += 1.0; }

        // how far this key is behind the sweeping arm
        let head = frac(t * p.speed);
        let d = frac((ang - head) * p.arms);

        // bright at the arm, fading along the trail behind it
        let v = 0.0;
        if d < p.trail {
            v = (1.0 - d / p.trail) ** 2.0;
        }

        out.push(hsv(ang + t * 0.1, 0.85, v * p.bright));
    }
    out
}
