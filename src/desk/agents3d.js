// Hard-surface 3D models for the six desk agents: chrome, glass and glowing
// cores, each shaped after its job and animated by the events it handles.
//
// Every builder returns
//   { root, port, tick(dt, heat, now), on(kind, info) }
// where `port` is the Object3D the beams dock onto.
import * as THREE from "three";

const TAU = Math.PI * 2;

/* ---------- materials ---------- */
const chrome = () => new THREE.MeshStandardMaterial({ color: 0x202020, metalness: 1, roughness: 0.2, envMapIntensity: 1.5 });
const darkMetal = () => new THREE.MeshStandardMaterial({ color: 0x0c0c0c, metalness: 0.85, roughness: 0.38, envMapIntensity: 1.0 });
const glass = (c) =>
  new THREE.MeshPhysicalMaterial({
    color: c, metalness: 0.1, roughness: 0.05, transparent: true, opacity: 0.3, depthWrite: false,
    iridescence: 0.8, iridescenceIOR: 1.35, clearcoat: 1, envMapIntensity: 1.8,
  });
// Emissive surfaces above the bloom threshold glow; colour stays saturated.
const glow = (c, k = 3) => new THREE.MeshStandardMaterial({ color: 0x000000, emissive: c, emissiveIntensity: k, roughness: 1, metalness: 0 });
const glowLine = (c, k = 2.5) => new THREE.LineBasicMaterial({ color: new THREE.Color(c).multiplyScalar(k), toneMapped: false, transparent: true, opacity: 0.95 });

function inlayRing(radius, color, tube = 0.006) {
  return new THREE.Mesh(new THREE.TorusGeometry(radius, tube, 8, 96), glow(color, 4));
}

function port(parent, x, y, z, color) {
  const p = new THREE.Object3D();
  p.position.set(x, y, z);
  // Small docking collar that the beams aim at.
  const collar = new THREE.Mesh(new THREE.TorusGeometry(0.06, 0.012, 8, 32), glow(color, 3));
  collar.rotation.x = Math.PI / 2;
  p.add(collar);
  parent.add(p);
  return p;
}

/* Shared impulse helper: decaying 0..1 value kicked by events. */
const impulse = () => ({ v: 0, kick(x = 1) { this.v = Math.min(1.5, this.v + x); }, step(dt, k = 3) { this.v *= Math.exp(-dt * k); return this.v; } });

/* ---------- SPOTTER: radar dish that aims at what it detects ---------- */
function spotter(color) {
  const root = new THREE.Group();
  const yaw = new THREE.Group();
  const pitch = new THREE.Group();
  root.add(yaw);
  yaw.add(pitch);

  const dishGeo = new THREE.SphereGeometry(0.44, 48, 16, 0, TAU, 0, Math.PI * 0.4);
  dishGeo.rotateX(-Math.PI / 2); // opening faces +Z
  const dishMat = chrome();
  dishMat.side = THREE.DoubleSide;
  const dish = new THREE.Mesh(dishGeo, dishMat);
  dish.position.z = 0.28;
  pitch.add(dish);
  const rim = inlayRing(0.42, color, 0.008);
  rim.position.z = 0.12;
  pitch.add(rim);
  const lens = new THREE.Mesh(new THREE.SphereGeometry(0.085, 24, 16), glow(color, 5));
  lens.position.z = -0.08;
  pitch.add(lens);
  const horn = new THREE.Mesh(new THREE.CylinderGeometry(0.012, 0.02, 0.42, 12), chrome());
  horn.rotation.x = Math.PI / 2;
  horn.position.z = 0.14;
  pitch.add(horn);
  const tip = port(pitch, 0, 0, 0.38, color);

  const mount = new THREE.Mesh(new THREE.TorusGeometry(0.5, 0.02, 12, 96), chrome());
  mount.rotation.x = Math.PI / 2;
  root.add(mount, inlayRing(0.5, color));
  root.children[root.children.length - 1].rotation.x = Math.PI / 2;
  const stem = new THREE.Mesh(new THREE.CylinderGeometry(0.03, 0.05, 0.5, 12), darkMetal());
  stem.position.y = -0.35;
  root.add(stem);

  // Scan cone from the horn towards the target.
  const coneGeo = new THREE.ConeGeometry(1, 1, 40, 1, true);
  coneGeo.translate(0, -0.5, 0);
  coneGeo.rotateX(-Math.PI / 2);
  // Light cone: bright at the dish, fading out towards the target.
  const coneU = { uColor: { value: new THREE.Color(color) }, uOpacity: { value: 0 } };
  const cone = new THREE.Mesh(coneGeo, new THREE.ShaderMaterial({
    uniforms: coneU, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide, toneMapped: false,
    vertexShader: `varying float vZ; void main(){ vZ = position.z; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }`,
    fragmentShader: `
      precision highp float;
      uniform vec3 uColor; uniform float uOpacity;
      varying float vZ;
      void main() { float f = pow(1.0 - clamp(vZ, 0.0, 1.0), 2.2); gl_FragColor = vec4(uColor * f * uOpacity, 1.0); }`,
  }));
  cone.position.z = 0.38;
  pitch.add(cone);

  const aim = new THREE.Vector3(0, 0, -1), want = new THREE.Vector3(0, 0, -1), tmp = new THREE.Vector3();
  const scan = impulse();
  let scanDist = 3;
  return {
    root, port: tip,
    tick(dt, heat, now) {
      const s = scan.step(dt, 1.8);
      // Idle: watch the signal source (world origin) with a slow sweep.
      if (s < 0.05) {
        tmp.set(0, 0, 0);
        root.worldToLocal(tmp).normalize();
        want.set(tmp.x + Math.sin(now * 0.5) * 0.35 * tmp.z, tmp.y, tmp.z - Math.sin(now * 0.5) * 0.35 * tmp.x).normalize();
      }
      aim.lerp(want, 1 - Math.exp(-dt * 5)).normalize();
      yaw.rotation.y = Math.atan2(aim.x, aim.z);
      pitch.rotation.x = -Math.asin(THREE.MathUtils.clamp(aim.y, -1, 1));
      coneU.uOpacity.value = Math.min(0.55, s * 0.55);
      cone.scale.set(0.06 * scanDist + 0.08, 0.06 * scanDist + 0.08, scanDist);
      lens.material.emissiveIntensity = 4 + heat * 6;
      mount.rotation.z += dt * (0.2 + heat);
    },
    on(kind, info) {
      if (kind !== "scan" || !info) return;
      // `info` is a world position: aim at it in the dish's own frame.
      tmp.copy(info);
      root.worldToLocal(tmp);
      scanDist = Math.max(0.5, tmp.length() - 0.4);
      want.copy(tmp).normalize();
      scan.kick(1);
    },
  };
}

/* ---------- TRACKER: gyroscope of nested rings ---------- */
function tracker(color) {
  const root = new THREE.Group();
  const rings = [0.48, 0.37, 0.27].map((r, i) => {
    const g = new THREE.Group();
    const ring = new THREE.Mesh(new THREE.TorusGeometry(r, 0.024, 16, 128), chrome());
    const inlay = inlayRing(r, color, 0.007);
    inlay.position.z = 0.02;
    g.add(ring, inlay);
    g.rotation.set(i * 0.7, i * 1.1, 0);
    root.add(g);
    return g;
  });
  const core = new THREE.Mesh(new THREE.SphereGeometry(0.1, 24, 16), glow(color, 5));
  const shell = new THREE.Mesh(new THREE.SphereGeometry(0.17, 32, 20), glass(color));
  root.add(core, shell);
  const top = port(root, 0, 0.6, 0, color);
  const burst = impulse();
  return {
    root, port: top,
    tick(dt, heat) {
      const b = burst.step(dt, 1.5);
      const sp = 1 + heat * 2 + b * 6;
      rings[0].rotation.y += dt * 0.5 * sp;
      rings[1].rotation.x += dt * 0.8 * sp;
      rings[2].rotation.z += dt * 1.2 * sp;
      rings[2].rotation.y += dt * 0.4 * sp;
      core.material.emissiveIntensity = 4 + heat * 6;
    },
    on() {
      burst.kick(1);
    },
  };
}

/* ---------- SIZER: nested cubes that rescale while computing ---------- */
function sizer(color) {
  const root = new THREE.Group();
  const outerGeo = new THREE.BoxGeometry(0.62, 0.62, 0.62);
  const outer = new THREE.Group();
  outer.add(new THREE.Mesh(outerGeo, glass(color)), new THREE.LineSegments(new THREE.EdgesGeometry(outerGeo), glowLine(color, 2.2)));
  const middle = new THREE.Mesh(new THREE.BoxGeometry(0.38, 0.38, 0.38), chrome());
  middle.rotation.set(Math.PI / 4, Math.PI / 4, 0);
  const inner = new THREE.Mesh(new THREE.BoxGeometry(0.16, 0.16, 0.16), glow(color, 5));
  root.add(outer, middle, inner);
  const top = port(root, 0, 0.55, 0, color);
  const pulse = impulse();
  return {
    root, port: top,
    tick(dt, heat, now) {
      const p = pulse.step(dt, 2.2);
      outer.rotation.y += dt * 0.3;
      outer.rotation.x += dt * 0.12;
      middle.rotation.y -= dt * (0.5 + heat);
      middle.rotation.z += dt * 0.2;
      // Out-of-phase breathing: outer grows while the middle shrinks.
      const w = Math.sin(now * 9) * p;
      outer.scale.setScalar(1 + 0.16 * Math.max(0, w) + p * 0.06);
      middle.scale.setScalar(1 - 0.18 * Math.max(0, w));
      inner.scale.setScalar(1 + p * 0.8);
      inner.material.emissiveIntensity = 4 + heat * 6;
    },
    on() {
      pulse.kick(1);
    },
  };
}

/* ---------- RISK: faceted cage with shield plates that open on a block ---------- */
function risk(color) {
  const root = new THREE.Group();
  const cageGeo = new THREE.IcosahedronGeometry(0.34, 0);
  root.add(new THREE.LineSegments(new THREE.EdgesGeometry(cageGeo), glowLine(color, 2)));
  root.add(new THREE.Mesh(cageGeo, glass(color)));
  const core = new THREE.Mesh(new THREE.OctahedronGeometry(0.13, 0), glow(color, 5));
  root.add(core);
  const plates = [];
  const plateMat = chrome();
  plateMat.side = THREE.DoubleSide;
  for (let i = 0; i < 4; i++) {
    const pivot = new THREE.Group();
    pivot.rotation.y = (i * TAU) / 4;
    const geo = new THREE.SphereGeometry(0.5, 24, 10, -0.5, 1.0, Math.PI * 0.28, Math.PI * 0.44);
    const plate = new THREE.Mesh(geo, plateMat);
    const edge = new THREE.LineSegments(new THREE.EdgesGeometry(geo, 30), glowLine(color, 1.6));
    const holder = new THREE.Group();
    holder.add(plate, edge);
    pivot.add(holder);
    root.add(pivot);
    plates.push(holder);
  }
  const top = port(root, 0, 0.62, 0, color);
  const open = impulse();
  const flash = impulse();
  return {
    root, port: top,
    tick(dt, heat) {
      const o = open.step(dt, 1.6);
      const f = flash.step(dt, 4);
      root.rotation.y += dt * 0.25;
      core.rotation.y += dt * (1 + heat * 3);
      core.rotation.x += dt * 0.6;
      // Plates face -X in their pivot: push them out radially and tilt open.
      for (const h of plates) {
        h.position.x = -o * 0.18;
        h.rotation.z = o * 0.35;
      }
      core.material.emissiveIntensity = 4 + heat * 4 + f * 12;
    },
    on(kind) {
      if (kind === "block") open.kick(1.2);
      flash.kick(kind === "block" ? 1 : 0.5);
    },
  };
}

/* ---------- EXECUTOR: crystal blade that thrusts when it fires ---------- */
function executor(color) {
  const root = new THREE.Group();
  const blade = new THREE.Group();
  const bladeGeo = new THREE.OctahedronGeometry(0.2, 0);
  bladeGeo.scale(1, 2.3, 1);
  blade.add(new THREE.Mesh(bladeGeo, glass(color)));
  blade.add(new THREE.LineSegments(new THREE.EdgesGeometry(bladeGeo), glowLine(color, 2.4)));
  const heart = new THREE.Mesh(new THREE.OctahedronGeometry(0.08, 0).scale(1, 2.2, 1), glow(color, 6));
  blade.add(heart);
  root.add(blade);
  const base = new THREE.Mesh(new THREE.TorusGeometry(0.34, 0.02, 12, 96), chrome());
  base.rotation.x = Math.PI / 2;
  base.position.y = -0.25;
  root.add(base);
  const shards = [];
  for (let i = 0; i < 3; i++) {
    const s = new THREE.Group();
    const geo = new THREE.TetrahedronGeometry(0.07, 0);
    s.add(new THREE.Mesh(geo, chrome()), new THREE.LineSegments(new THREE.EdgesGeometry(geo), glowLine(color, 2)));
    root.add(s);
    shards.push(s);
  }
  const top = port(blade, 0, 0.5, 0, color);
  const thrust = impulse();
  const dir = new THREE.Vector3(1, 0, 0), tmp = new THREE.Vector3();
  return {
    root, port: top,
    tick(dt, heat, now) {
      const t = thrust.step(dt, 3.2);
      blade.rotation.y += dt * (0.6 + heat * 2);
      // Lunge towards the target, then spring back.
      blade.position.copy(dir).multiplyScalar(Math.sin(Math.min(1, t) * Math.PI) * 0.22);
      shards.forEach((s, i) => {
        const a = now * (0.9 + heat) + (i * TAU) / 3;
        const r = 0.42 + t * 0.25;
        s.position.set(Math.cos(a) * r, Math.sin(now * 1.3 + i) * 0.08, Math.sin(a) * r);
        s.rotation.x += dt * 2;
        s.rotation.y += dt * 1.4;
      });
      heart.material.emissiveIntensity = 5 + heat * 6 + t * 8;
    },
    on(kind, info) {
      if (info) {
        tmp.copy(info);
        root.worldToLocal(tmp);
        dir.copy(tmp).setY(0).normalize();
      }
      thrust.kick(1);
    },
  };
}

/* ---------- LEDGER: a coin stack that grows with every win ---------- */
function ledger(color, colors) {
  const root = new THREE.Group();
  const stack = new THREE.Group();
  root.add(stack);
  const coinGeo = new THREE.CylinderGeometry(0.28, 0.28, 0.055, 48);
  const coinMat = chrome();
  const coins = [];
  const H = 0.072, BASE = -0.3, MAX = 9;
  function addCoin(drop) {
    const c = new THREE.Group();
    const rim = inlayRing(0.28, color, 0.007);
    rim.rotation.x = Math.PI / 2;
    c.add(new THREE.Mesh(coinGeo, coinMat), rim);
    c.userData.y = BASE + coins.length * H;
    c.position.y = drop ? c.userData.y + 0.9 : c.userData.y;
    c.rotation.y = coins.length * 0.4;
    stack.add(c);
    coins.push(c);
    return c;
  }
  for (let i = 0; i < 4; i++) addCoin(false);
  const top = port(root, 0, 0.55, 0, color);
  const flash = impulse();
  const lossColor = new THREE.Color(colors.short);
  const base = new THREE.Color(color);
  return {
    root, port: top,
    tick(dt, heat) {
      const f = flash.step(dt, 2.5);
      stack.rotation.y += dt * (0.3 + heat);
      for (const c of coins) c.position.y += (c.userData.y - c.position.y) * (1 - Math.exp(-dt * 7));
      for (const c of coins) {
        c.children[1].material.emissive.copy(base);
        c.children[1].material.emissiveIntensity = 3;
      }
      const topCoin = coins[coins.length - 1];
      topCoin.children[1].material.emissive.copy(f > 0.05 && flash.loss ? lossColor : base);
      topCoin.children[1].material.emissiveIntensity = 4 + heat * 4 + f * 8;
      top.position.y = BASE + coins.length * H + 0.25;
    },
    on(kind, value) {
      if (kind !== "pnl") return flash.kick(0.4);
      flash.loss = value < 0;
      if (value > 0) {
        if (coins.length >= MAX) {
          // Bank the stack: keep the base, start counting up again.
          for (const c of coins.splice(4)) stack.remove(c);
        }
        addCoin(true);
      }
      flash.kick(1);
    },
  };
}

const BUILDERS = { spotter, tracker, sizer, risk, executor, ledger };

/** Build the model for agent `id` in `color`. `colors` holds long/short. */
export function buildAgentModel(id, color, colors) {
  return BUILDERS[id](color, colors);
}

/* ---------- hex pad with a segmented status ring ---------- */
export function buildPad(color) {
  const g = new THREE.Group();
  const hexGeo = new THREE.CylinderGeometry(0.72, 0.8, 0.07, 6);
  g.add(new THREE.Mesh(hexGeo, darkMetal()));
  const edge = new THREE.LineSegments(new THREE.EdgesGeometry(hexGeo), glowLine(color, 1.6));
  g.add(edge);
  const u = { uTime: { value: 0 }, uActive: { value: 0 }, uProgress: { value: 0 }, uWarn: { value: 0 }, uColor: { value: new THREE.Color(color) } };
  const ring = new THREE.Mesh(
    new THREE.RingGeometry(0.88, 0.96, 128, 1),
    new THREE.ShaderMaterial({
      uniforms: u,
      transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide, toneMapped: false,
      vertexShader: `varying vec2 vP; void main(){ vP = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }`,
      fragmentShader: `
        precision highp float;
        uniform float uTime, uActive, uProgress, uWarn;
        uniform vec3 uColor;
        varying vec2 vP;
        void main() {
          float a = atan(vP.y, vP.x) / 6.28318 + 0.5;
          float seg = fract(a * 36.0 - uTime * (0.08 + uActive * 0.6 + uWarn * 1.2));
          float dash = smoothstep(0.0, 0.06, seg) * smoothstep(0.66, 0.58, seg);
          float arc = step(a, uProgress) * uActive;
          // Warning: the ring turns red and beats.
          float beat = uWarn * (0.6 + 0.4 * sin(uTime * 12.0));
          vec3 col = mix(uColor, vec3(1.0, 0.1, 0.18), clamp(uWarn * 1.5, 0.0, 1.0));
          float k = 0.12 + dash * (0.35 + uActive * 0.9 + beat) + arc * 1.8 + beat * 0.5;
          gl_FragColor = vec4(col * k, 1.0);
        }`,
    }),
  );
  ring.rotation.x = -Math.PI / 2;
  ring.position.y = 0.04;
  g.add(ring);
  // Soft light pool on the pad.
  const pool = new THREE.Mesh(
    new THREE.CircleGeometry(0.7, 48),
    new THREE.ShaderMaterial({
      uniforms: { uColor: u.uColor, uActive: u.uActive },
      transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false,
      vertexShader: `varying vec2 vP; void main(){ vP = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }`,
      fragmentShader: `
        precision highp float;
        uniform vec3 uColor; uniform float uActive;
        varying vec2 vP;
        void main() {
          float r = length(vP) / 0.7;
          gl_FragColor = vec4(uColor * exp(-r * r * 4.0) * (0.35 + uActive * 0.6), 1.0);
        }`,
    }),
  );
  pool.rotation.x = -Math.PI / 2;
  pool.position.y = 0.041;
  g.add(pool);
  let progressT = 1;
  return {
    root: g,
    tick(dt, heat, now) {
      u.uTime.value = now;
      u.uActive.value = Math.min(1, heat);
      if (progressT < 1) progressT = Math.min(1, progressT + dt / 0.9);
      u.uProgress.value = progressT;
    },
    start() {
      progressT = 0;
    },
    setWarn(v) {
      u.uWarn.value = v;
    },
  };
}
