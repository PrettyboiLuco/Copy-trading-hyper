// 3D orbit for the Copy Desk: the wallet as a glittering light organism at
// the centre, our six agents as smaller organisms on a ring, the wallet's
// positions as satellites, and comet-like particle beams that carry every
// pipeline step (plus ambient "scan" chatter) between them.
//
// Look built with GetLayers Scene Lab references: the particle organisms
// (glint/bokeh billboards, depth of field, shock front) follow "Sea Sparkle",
// the chasing light around the agent ring follows "Ring Galaxy".
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { EffectComposer } from "three/addons/postprocessing/EffectComposer.js";
import { RenderPass } from "three/addons/postprocessing/RenderPass.js";
import { UnrealBloomPass } from "three/addons/postprocessing/UnrealBloomPass.js";
import { OutputPass } from "three/addons/postprocessing/OutputPass.js";

const CONFIG = {
  background: "#03050a",
  coreHot: "#fff4e6",
  coreWarm: "#ffb547",
  coreBody: "#5cc8ff",
  coreDeep: "#1b3a8f",
  ringMain: "#5cc8ff",
  ringGlow: "#b9a8ff",
  scan: "#5cc8ff",
  ringRadius: 7,
  coreParticles: 4200,
  agentParticles: 750,
  satParticles: 220,
  diskParticles: 4200,
  ringParticles: 3200,
  mobileScale: 0.5,
  coreSpikes: 12,
  agentSpikes: 7,
  particleSize: 0.3,
  bokeh: 0.05,
  bokehSize: 1.4,
  twinkle: 5,
  // Organisms pack thousands of additive glints into few pixels: keep each one dim.
  brightness: 0.13,
  diskBrightness: 0.4,
  beamBrightness: 0.55,
  beamTrail: 46,
  beamSize: 1.0,
  ringSweepSpeed: 1.6,
  ringSweepArms: 3,
  bloomStrength: 0.6,
  bloomRadius: 0.45,
  bloomThreshold: 0.72,
  autoRotate: 0.3,
  ambientBeamsPerSec: 7,
  scanEverySec: 3.2,
};

const POOL = 110;

const additive = (opts) => ({ transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, ...opts });
const rand = (a, b) => a + Math.random() * (b - a);
const pick = (arr) => arr[(Math.random() * arr.length) | 0];
const gauss = () => Math.random() + Math.random() + Math.random() + Math.random() - 2;

/* ---------- shared glint/bokeh particle material (Sea Sparkle technique) ---------- */
const PARTICLE_VS = `
  attribute vec3 aColor;
  attribute float aSize, aSeed, aBokeh, aDist;
  uniform float uTime, uSize, uPixelRatio, uEnergy, uFocal, uDof, uDofSize, uBreath;
  varying vec3 vColor;
  varying float vSeed, vBlur, vDist;
  void main() {
    vColor = aColor; vSeed = aSeed; vDist = aDist;
    vec3 p = position;
    float ph = aSeed * 6.2831;
    p *= 1.0 + uBreath * sin(uTime * 0.8 + aDist * 3.0) + uEnergy * 0.18 * aDist;
    p += vec3(sin(uTime * 0.6 + ph), cos(uTime * 0.5 + ph * 1.3), sin(uTime * 0.7 + ph * 0.7)) * 0.012;
    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    float depth = -mv.z;
    vBlur = max(clamp(abs(depth - uFocal) * uDof, 0.0, 1.0), aBokeh);
    float s = uSize * aSize * uPixelRatio * (300.0 / max(0.001, depth));
    s *= 1.0 + vBlur * uDofSize;
    s *= 1.0 + uEnergy * 0.5;
    gl_PointSize = s;
    gl_Position = projectionMatrix * mv;
  }`;
const PARTICLE_FS = `
  precision highp float;
  uniform float uTime, uTwinkle, uBright, uEnergy, uShock, uShockAmp;
  varying vec3 vColor;
  varying float vSeed, vBlur, vDist;
  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    if (r > 1.0) discard;
    float glint = exp(-r * r * 5.0);
    float disc = smoothstep(1.0, 0.5, r);
    float rim = smoothstep(0.45, 0.92, r) * (1.0 - smoothstep(0.9, 1.0, r));
    float shape = mix(glint, disc * 0.55 + rim * 0.8, vBlur);
    float tw = 0.55 + 0.45 * sin(uTime * uTwinkle + vSeed * 6.2831);
    float sd = (vDist - uShock) * 7.0;
    float shock = uShock >= 0.0 ? uShockAmp * exp(-sd * sd) * 2.4 : 0.0;
    vec3 c = vColor * (0.6 + tw * 0.6) * (1.0 + uEnergy * 0.9 + shock) * uBright;
    gl_FragColor = vec4(c, shape);
  }`;

function particleMaterial(overrides = {}) {
  return new THREE.ShaderMaterial({
    uniforms: {
      uTime: { value: 0 },
      uSize: { value: CONFIG.particleSize },
      uPixelRatio: { value: 1 },
      uEnergy: { value: 0 },
      uFocal: { value: 18 },
      uDof: { value: CONFIG.bokeh },
      uDofSize: { value: CONFIG.bokehSize },
      uBreath: { value: 0.035 },
      uTwinkle: { value: CONFIG.twinkle },
      uBright: { value: CONFIG.brightness },
      uShock: { value: -1 },
      uShockAmp: { value: 0 },
      ...overrides,
    },
    vertexShader: PARTICLE_VS,
    fragmentShader: PARTICLE_FS,
    ...additive({ depthTest: true }),
  });
}

function setAttrs(geo, n) {
  const a = {
    position: new Float32Array(n * 3),
    aColor: new Float32Array(n * 3),
    aSize: new Float32Array(n),
    aSeed: new Float32Array(n),
    aBokeh: new Float32Array(n),
    aDist: new Float32Array(n),
  };
  geo.setAttribute("position", new THREE.BufferAttribute(a.position, 3));
  geo.setAttribute("aColor", new THREE.BufferAttribute(a.aColor, 3));
  for (const k of ["aSize", "aSeed", "aBokeh", "aDist"]) geo.setAttribute(k, new THREE.BufferAttribute(a[k], 1));
  return a;
}

/**
 * A glittering "organism": dense hot core, radial spikes and a bokeh haze.
 * Colours run hot (centre) -> warm -> body -> deep (outside).
 */
function organism({ count, coreR, spikes, spikeLen, cone, hot, warm, body, deep }) {
  const geo = new THREE.BufferGeometry();
  const a = setAttrs(geo, count);
  const C = { hot: new THREE.Color(hot), warm: new THREE.Color(warm), body: new THREE.Color(body), deep: new THREE.Color(deep) };
  const maxR = coreR + spikeLen * 1.2;
  const dirs = [];
  for (let i = 0; i < spikes; i++) {
    // Fibonacci sphere with jitter, so spikes spread evenly but not rigidly.
    const y = 1 - ((i + 0.5) / spikes) * 2, rr = Math.sqrt(1 - y * y), th = i * 2.39996 + rand(-0.3, 0.3);
    dirs.push(new THREE.Vector3(Math.cos(th) * rr, y, Math.sin(th) * rr).normalize());
  }
  const p = new THREE.Vector3(), perp = new THREE.Vector3(), c = new THREE.Color();
  const nSpike = spikes ? Math.floor(count * 0.38) : 0, nHaze = Math.floor(count * 0.12);
  for (let i = 0; i < count; i++) {
    let bokeh = 0, dim = 1;
    if (i < nSpike) {
      const d = dirs[i % spikes];
      const t = Math.pow(Math.random(), 1.7);
      perp.set(gauss(), gauss(), gauss()).cross(d).normalize().multiplyScalar(Math.abs(gauss()) * cone * (0.15 + t) * spikeLen * 0.5);
      p.copy(d).multiplyScalar(coreR * 0.6 + t * spikeLen).add(perp);
      dim = 1.1 - t * 0.5;
    } else if (i < nSpike + nHaze) {
      p.randomDirection().multiplyScalar(coreR * rand(1.1, 3.2));
      bokeh = Math.random() < 0.6 ? 1 : 0;
      dim = 0.45;
    } else {
      p.randomDirection().multiplyScalar(coreR * Math.pow(Math.random(), 0.6));
    }
    const rn = Math.min(1, p.length() / maxR);
    const heat = Math.exp(-Math.pow(p.length() / (coreR * 0.9), 2));
    c.copy(C.body).lerp(C.deep, THREE.MathUtils.smoothstep(rn, 0.25, 0.9));
    c.lerp(C.warm, Math.min(0.9, heat * 0.9));
    c.lerp(C.hot, Math.max(0, heat - 0.5) * 1.6);
    let bright = THREE.MathUtils.lerp(1.5, 0.5, rn) * dim;
    if (Math.random() < 0.05) {
      c.set(0xffffff);
      bright *= 1.3;
    }
    a.position.set([p.x, p.y, p.z], i * 3);
    a.aColor.set([c.r * bright, c.g * bright, c.b * bright], i * 3);
    a.aSize[i] = (0.55 + Math.random() * 0.6) * (bokeh ? 1.6 : 1);
    a.aSeed[i] = Math.random();
    a.aBokeh[i] = bokeh;
    a.aDist[i] = rn;
  }
  const mat = particleMaterial();
  const points = new THREE.Points(geo, mat);
  points.frustumCulled = false;
  return { points, u: mat.uniforms, shockT: 2 };
}

/* ---------- soft hot haze billboard (Sea Sparkle core haze) ---------- */
function haze(color, hotColor, size) {
  const mat = new THREE.ShaderMaterial({
    uniforms: { uWarm: { value: new THREE.Color(color) }, uHot: { value: new THREE.Color(hotColor) }, uGlow: { value: 0.5 }, uTime: { value: 0 } },
    vertexShader: `varying vec2 vUv; void main(){ vUv = uv; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }`,
    fragmentShader: `
      precision highp float;
      uniform vec3 uWarm, uHot; uniform float uGlow, uTime;
      varying vec2 vUv;
      void main() {
        float r = length(vUv - 0.5) * 2.0;
        float hot = exp(-r * r * 9.0);
        float halo = exp(-r * 3.4) * 0.35;
        float flick = 0.92 + 0.08 * sin(uTime * 1.7) + 0.04 * sin(uTime * 4.3);
        vec3 c = (uHot * hot + uWarm * halo) * uGlow * flick;
        gl_FragColor = vec4(c, clamp(hot + halo, 0.0, 1.0));
      }`,
    ...additive({ depthTest: true }),
  });
  const m = new THREE.Mesh(new THREE.PlaneGeometry(size, size), mat);
  return m;
}

export function createOrbit3D(container, opts) {
  const canvas = document.createElement("canvas");
  canvas.className = "orbit3d";
  let renderer;
  try {
    renderer = new THREE.WebGLRenderer({ canvas, antialias: true, powerPreference: "high-performance" });
  } catch (e) {
    console.warn("WebGL nicht verfügbar", e);
    return null;
  }
  const mobile = matchMedia("(max-width: 760px)").matches;
  const coarse = matchMedia("(pointer: coarse)").matches;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const q = mobile ? CONFIG.mobileScale : 1;
  const pixelRatio = Math.min(devicePixelRatio || 1, mobile ? 1.5 : 2);
  renderer.setPixelRatio(pixelRatio);
  renderer.setClearColor(CONFIG.background);
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.0;
  container.prepend(canvas);

  const labels = document.createElement("div");
  labels.className = "labels3d";
  container.appendChild(labels);

  const scene = new THREE.Scene();
  scene.fog = new THREE.FogExp2(CONFIG.background, 0.02);
  const camera = new THREE.PerspectiveCamera(42, 1, 0.1, 300);
  camera.position.set(0, 7.2, 16.5);

  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.enablePan = false;
  controls.enableZoom = false;
  controls.autoRotate = !reduced;
  controls.autoRotateSpeed = CONFIG.autoRotate;
  controls.minPolarAngle = 0.55;
  controls.maxPolarAngle = 1.45;
  // On phones a drag must keep scrolling the page.
  controls.enabled = !coarse;

  const composer = new EffectComposer(renderer);
  composer.addPass(new RenderPass(scene, camera));
  const bloom = new UnrealBloomPass(new THREE.Vector2(256, 256), CONFIG.bloomStrength, CONFIG.bloomRadius, CONFIG.bloomThreshold);
  composer.addPass(bloom);
  composer.addPass(new OutputPass());

  const col = (hex) => new THREE.Color(hex);
  const materials = []; // every particle material, for shared per-frame uniforms
  const track = (o) => (materials.push(o.u), o);
  const world = new THREE.Group();
  world.rotation.z = 0.1;
  scene.add(world);
  const billboards = [];

  /* ---------- stars ---------- */
  {
    const n = 900;
    const geo = new THREE.BufferGeometry();
    const a = setAttrs(geo, n);
    const v = new THREE.Vector3();
    for (let i = 0; i < n; i++) {
      v.randomDirection().multiplyScalar(rand(45, 110));
      a.position.set([v.x, v.y, v.z], i * 3);
      const k = rand(0.15, 0.55);
      a.aColor.set([0.6 * k, 0.72 * k, 1.0 * k], i * 3);
      a.aSize[i] = rand(0.8, 2.2);
      a.aSeed[i] = Math.random();
    }
    const mat = particleMaterial({ uDof: { value: 0 }, uBreath: { value: 0 }, uTwinkle: { value: 1.3 }, uBright: { value: 0.8 } });
    materials.push(mat.uniforms);
    scene.add(new THREE.Points(geo, mat));
  }

  /* ---------- the wallet: a large organism ---------- */
  const core = track(organism({
    count: Math.round(CONFIG.coreParticles * q), coreR: 0.62, spikes: CONFIG.coreSpikes, spikeLen: 1.9, cone: 0.24,
    hot: CONFIG.coreHot, warm: CONFIG.coreWarm, body: CONFIG.coreBody, deep: CONFIG.coreDeep,
  }));
  world.add(core.points);
  const coreHaze = haze(CONFIG.coreWarm, CONFIG.coreHot, 3.4);
  world.add(coreHaze);
  billboards.push(coreHaze);
  const coreWire = new THREE.LineSegments(
    new THREE.EdgesGeometry(new THREE.IcosahedronGeometry(1.25, 1)),
    new THREE.LineBasicMaterial(additive({ color: CONFIG.coreBody, opacity: 0.12 })),
  );
  world.add(coreWire);
  let coreHeat = 0;

  /* ---------- accretion disk: particles + tangled threads ---------- */
  const palette = Object.values(opts.colors).map(col);
  const N = Math.round(CONFIG.diskParticles * q);
  const disk = new THREE.BufferGeometry();
  const D = setAttrs(disk, N);
  const pR = new Float32Array(N), pA = new Float32Array(N), pY = new Float32Array(N), pW = new Float32Array(N);
  for (let i = 0; i < N; i++) {
    const r = 2.4 + Math.pow(Math.random(), 0.8) * 7.2;
    pR[i] = r;
    pA[i] = Math.random() * Math.PI * 2;
    pY[i] = gauss() * 0.5 * (0.1 + r * 0.04);
    pW[i] = (0.35 / Math.sqrt(r)) * rand(0.7, 1.3);
    const c = Math.random() < 0.55 ? palette[0] : pick(palette);
    const k = rand(0.12, 0.5);
    D.aColor.set([c.r * k, c.g * k, c.b * k], i * 3);
    D.aSize[i] = rand(0.35, 0.8);
    D.aSeed[i] = Math.random();
    D.aBokeh[i] = Math.random() < 0.015 ? 1 : 0;
  }
  const diskMat = particleMaterial({ uBreath: { value: 0 }, uBright: { value: CONFIG.diskBrightness }, uDof: { value: CONFIG.bokeh * 0.35 } });
  materials.push(diskMat.uniforms);
  const diskPts = new THREE.Points(disk, diskMat);
  diskPts.frustumCulled = false;
  world.add(diskPts);

  const threads = [];
  for (let i = 0; i < 40; i++) {
    const r = rand(2.6, 8.8), squash = rand(0.55, 1), pts = [];
    for (let k = 0; k <= 160; k++) {
      const ang = (k / 160) * Math.PI * 2;
      pts.push(new THREE.Vector3(Math.cos(ang) * r, Math.sin(ang * 3 + i) * 0.12, Math.sin(ang) * r * squash));
    }
    const line = new THREE.Line(
      new THREE.BufferGeometry().setFromPoints(pts),
      new THREE.LineBasicMaterial(additive({ color: pick(palette), opacity: rand(0.03, 0.09) })),
    );
    line.rotation.set(rand(-0.35, 0.35), rand(0, Math.PI), rand(-0.25, 0.25));
    line.userData.w = rand(0.02, 0.09) * (Math.random() < 0.5 ? -1 : 1);
    world.add(line);
    threads.push(line);
  }

  /* ---------- agent ring with chasing light (Ring Galaxy technique) ---------- */
  const ringU = {
    uTime: { value: 0 }, uPixelRatio: { value: pixelRatio },
    uMain: { value: col(CONFIG.ringMain) }, uGlow: { value: col(CONFIG.ringGlow) },
    uSweep: { value: CONFIG.ringSweepSpeed }, uArms: { value: CONFIG.ringSweepArms }, uPulse: { value: 0 },
  };
  {
    const n = Math.round(CONFIG.ringParticles * q);
    const geo = new THREE.BufferGeometry();
    const pos = new Float32Array(n * 3), ang = new Float32Array(n), band = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const t = Math.random() * Math.PI * 2, b = gauss() * 0.5;
      const r = CONFIG.ringRadius + b * 0.22;
      pos.set([Math.cos(t) * r, gauss() * 0.025, Math.sin(t) * r], i * 3);
      ang[i] = t;
      band[i] = Math.max(-1, Math.min(1, b));
    }
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setAttribute("aAngle", new THREE.BufferAttribute(ang, 1));
    geo.setAttribute("aBand", new THREE.BufferAttribute(band, 1));
    const mat = new THREE.ShaderMaterial({
      uniforms: ringU,
      vertexShader: `
        attribute float aAngle, aBand;
        uniform float uPixelRatio;
        varying float vAngle, vBand;
        void main() {
          vAngle = aAngle; vBand = aBand;
          vec4 mv = modelViewMatrix * vec4(position, 1.0);
          gl_PointSize = (1.2 + (1.0 - abs(aBand)) * 1.6) * uPixelRatio * (16.0 / max(1.0, -mv.z));
          gl_Position = projectionMatrix * mv;
        }`,
      fragmentShader: `
        precision highp float;
        uniform float uTime, uSweep, uArms, uPulse;
        uniform vec3 uMain, uGlow;
        varying float vAngle, vBand;
        void main() {
          float across = 1.0 - pow(abs(vBand), 2.4);
          float sweep = pow(0.5 + 0.5 * sin(vAngle * uArms - uTime * uSweep), 6.0);
          vec3 c = mix(uMain * 0.35, uGlow, sweep) * (0.35 + across * (0.9 + sweep * 2.2 + uPulse));
          float tex = 1.0 - smoothstep(0.4, 1.0, length(2.0 * gl_PointCoord - 1.0));
          gl_FragColor = vec4(c * tex, tex);
        }`,
      ...additive({ depthTest: true }),
    });
    const ring = new THREE.Points(geo, mat);
    ring.frustumCulled = false;
    world.add(ring);
  }

  /* ---------- radar sweep + scan rings ---------- */
  const sweep = new THREE.Mesh(
    new THREE.CircleGeometry(CONFIG.ringRadius + 1.6, 48, 0, 0.55),
    new THREE.MeshBasicMaterial(additive({ color: CONFIG.scan, opacity: 0.03, side: THREE.DoubleSide })),
  );
  sweep.rotation.x = -Math.PI / 2;
  world.add(sweep);
  const scans = Array.from({ length: 5 }, () => {
    const m = new THREE.Mesh(
      new THREE.RingGeometry(0.994, 1.0, 160),
      new THREE.MeshBasicMaterial(additive({ color: CONFIG.scan, opacity: 0, side: THREE.DoubleSide })),
    );
    m.rotation.x = -Math.PI / 2;
    m.userData.t = 2;
    world.add(m);
    return m;
  });
  const fireScan = (color = CONFIG.scan) => {
    const s = scans.find((m) => m.userData.t >= 1);
    if (!s) return;
    s.userData.t = 0;
    s.material.color.set(color);
  };

  /* ---------- agents: smaller organisms in their own colour ---------- */
  const agents = new Map();
  opts.agents.forEach((ag, i) => {
    const ang = -Math.PI / 2 - Math.PI / 3 + (i * Math.PI) / 3;
    const c = col(opts.colors[ag.id]);
    const g = new THREE.Group();
    const home = new THREE.Vector3(Math.cos(ang) * CONFIG.ringRadius, 0, Math.sin(ang) * CONFIG.ringRadius);
    g.position.copy(home);
    const warm = c.clone().lerp(new THREE.Color(0xffffff), 0.45);
    const org = track(organism({
      count: Math.round(CONFIG.agentParticles * q), coreR: 0.24, spikes: CONFIG.agentSpikes, spikeLen: 0.78, cone: 0.3,
      hot: "#ffffff", warm: "#" + warm.getHexString(), body: opts.colors[ag.id], deep: "#" + c.clone().multiplyScalar(0.3).getHexString(),
    }));
    const hz = haze(opts.colors[ag.id], "#ffffff", 1.5);
    billboards.push(hz);
    // Faint pylon down to the disk plane.
    const pylon = new THREE.Line(
      new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(0, -0.5, 0), new THREE.Vector3(0, -2.2, 0)]),
      new THREE.LineBasicMaterial(additive({ color: c, opacity: 0.18 })),
    );
    g.add(org.points, hz, pylon);
    world.add(g);
    const el = document.createElement("div");
    el.className = "lab3 ag";
    el.style.setProperty("--c", opts.colors[ag.id]);
    el.innerHTML = `<b>${ag.name}</b><span>–</span>`;
    labels.appendChild(el);
    agents.set(ag.id, { g, org, hz, home, phase: i * 1.1, heat: 0, el, count: el.querySelector("span") });
  });

  /* ---------- pipeline links (faint arcs that light up with traffic) ---------- */
  const links = [];
  const linkKey = (a, b) => [a, b].sort().join(">");
  const linkMap = new Map();
  const addLink = (a, b) => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(33 * 3), 3));
    const line = new THREE.Line(geo, new THREE.LineBasicMaterial(additive({ color: 0x8fb4ff, opacity: 0.06 })));
    world.add(line);
    const l = { a, b, line, heat: 0 };
    links.push(l);
    linkMap.set(linkKey(a, b), l);
  };
  const ids = opts.agents.map((a) => a.id);
  ids.forEach((id, i) => addLink(id, ids[(i + 1) % ids.length]));
  addLink("center", "spotter");
  addLink("center", "tracker");

  /* ---------- satellites (wallet positions) ---------- */
  const sats = new Map();
  function satellite(p) {
    const color = p.size > 0 ? opts.long : opts.short;
    const c = col(color);
    const pivot = new THREE.Group();
    pivot.rotation.set(rand(-0.5, 0.5), rand(0, Math.PI * 2), rand(-0.35, 0.35));
    const org = track(organism({
      count: Math.round(CONFIG.satParticles * q), coreR: 0.5, spikes: 0, spikeLen: 0, cone: 0,
      hot: "#ffffff", warm: "#" + c.clone().lerp(new THREE.Color(0xffffff), 0.4).getHexString(), body: color, deep: "#" + c.clone().multiplyScalar(0.35).getHexString(),
    }));
    const hz = haze(color, "#ffffff", 1.6);
    billboards.push(hz);
    const orbitR = rand(2.5, 4.6);
    const orbitLine = new THREE.Mesh(
      new THREE.RingGeometry(orbitR - 0.008, orbitR + 0.008, 128),
      new THREE.MeshBasicMaterial(additive({ color: c, opacity: 0.08, side: THREE.DoubleSide })),
    );
    orbitLine.rotation.x = -Math.PI / 2;
    const body = new THREE.Group();
    body.add(org.points, hz);
    pivot.add(orbitLine, body);
    world.add(pivot);
    const tether = new THREE.Line(
      new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]),
      new THREE.LineBasicMaterial(additive({ color: c, opacity: 0.18 })),
    );
    world.add(tether);
    const el = document.createElement("div");
    el.className = "lab3 sat";
    labels.appendChild(el);
    return { pivot, body, org, hz, orbitLine, tether, el, r: orbitR, a: rand(0, Math.PI * 2), w: rand(0.12, 0.22), born: performance.now(), size: 0.2, heat: 0 };
  }
  const tmp = new THREE.Vector3();
  const satWorldPos = (s, out) => out.set(Math.cos(s.a) * s.r, 0, Math.sin(s.a) * s.r).applyEuler(s.pivot.rotation);

  /* ---------- beams: comets made of glint particles ---------- */
  const T = CONFIG.beamTrail;
  const beamGeo = new THREE.BufferGeometry();
  const BA = setAttrs(beamGeo, POOL * T);
  for (let i = 0; i < POOL * T; i++) {
    BA.aSeed[i] = Math.random();
    BA.aDist[i] = 0;
  }
  const beamMat = particleMaterial({ uDof: { value: CONFIG.bokeh * 0.6 }, uBreath: { value: 0 }, uTwinkle: { value: 9 }, uBright: { value: CONFIG.beamBrightness } });
  materials.push(beamMat.uniforms);
  const beamPts = new THREE.Points(beamGeo, beamMat);
  beamPts.frustumCulled = false;
  world.add(beamPts);
  const jitter = Array.from({ length: T }, () => new THREE.Vector3(gauss(), gauss(), gauss()).multiplyScalar(0.5));
  const beams = Array.from({ length: POOL }, () => ({ active: false }));
  const A = new THREE.Vector3(), B = new THREE.Vector3(), C = new THREE.Vector3(), P = new THREE.Vector3();
  const WHITE = new THREE.Color(0xffffff);

  function posOf(key, out) {
    if (key === "center") return out.set(0, 0, 0);
    if (key.startsWith("sat:")) {
      const s = sats.get(key.slice(4));
      return s ? satWorldPos(s, out) : out.set(0, 0, 0);
    }
    const a = agents.get(key);
    return a ? out.copy(a.g.position) : out.set(0, 0, 0);
  }

  function fire(from, to, color, o = {}) {
    const b = beams.find((x) => !x.active);
    if (!b) return;
    Object.assign(b, {
      active: true, from, to, color: col(color), t: 0,
      dur: o.dur ?? rand(0.9, 1.5), trail: o.trail ?? 0.3, power: o.power ?? 0.5,
      lift: o.lift ?? rand(0.8, 2.6), size: o.size ?? 0.7, onArrive: o.onArrive, spray: o.spray ?? 0.05,
    });
    const l = linkMap.get(linkKey(from.startsWith("sat:") ? "center" : from, to.startsWith("sat:") ? "center" : to));
    if (l) l.heat = Math.min(1, l.heat + b.power * 0.6);
  }

  function curvePoint(b, t, out) {
    posOf(b.from, A);
    posOf(b.to, B);
    C.copy(A).add(B).multiplyScalar(0.5).multiplyScalar(0.75);
    C.y += b.lift + A.distanceTo(B) * 0.12;
    const u = 1 - t;
    return out.set(
      u * u * A.x + 2 * u * t * C.x + t * t * B.x,
      u * u * A.y + 2 * u * t * C.y + t * t * B.y,
      u * u * A.z + 2 * u * t * C.z + t * t * B.z,
    );
  }

  function shock(org, amp = 1) {
    org.shockT = 0;
    org.u.uShockAmp.value = amp;
  }

  function pulseAgent(id, power = 1) {
    const a = agents.get(id);
    if (!a) return;
    a.heat = Math.min(1.4, a.heat + power);
    shock(a.org, Math.min(1.2, power));
  }

  /* ---------- ambient chatter: scans and messages between agents ---------- */
  let chatterHeat = 0;
  function ambient() {
    const r = Math.random();
    const ag = pick(ids);
    const c = opts.colors[ag];
    const satKeys = [...sats.keys()];
    if (r < 0.34) {
      const i = ids.indexOf(ag);
      fire(ag, ids[(i + 1) % ids.length], c, { power: rand(0.3, 0.55) });
    } else if (r < 0.58) {
      // scan: agent pings the core, the core answers
      fire(ag, "center", c, { power: rand(0.3, 0.5), onArrive: () => Math.random() < 0.6 && fire("center", ag, CONFIG.scan, { power: 0.35 }) });
    } else if (r < 0.75 && satKeys.length) {
      const s = pick(satKeys);
      fire("sat:" + s, pick(["spotter", "tracker"]), opts.colors.spotter, { power: rand(0.3, 0.5), lift: rand(0.4, 1.4) });
    } else {
      let other = pick(ids);
      if (other === ag) other = ids[(ids.indexOf(ag) + 3) % ids.length];
      fire(ag, other, c, { power: rand(0.25, 0.45), lift: rand(1.5, 3.5) });
    }
  }

  /* ---------- events from the bot ---------- */
  const FLOW = opts.flow;
  function coinIn(text) {
    for (const k of sats.keys()) if (new RegExp(`\\b${k}\\b`).test(text)) return k;
    return null;
  }
  function onActivity(a, delay) {
    setTimeout(() => {
      let [from, to] = FLOW[a.agent] || ["center", a.agent];
      const coin = coinIn(a.text || "");
      if ((a.agent === "spotter" || a.agent === "tracker") && coin) from = "sat:" + coin;
      const big = a.level === "trade" || a.level === "pnl";
      const color = opts.colors[a.agent];
      chatterHeat = Math.min(1, chatterHeat + 0.25);
      fire(from, to, color, {
        power: big ? 1.6 : 1.05, dur: big ? 1.25 : 1.0, trail: 0.42, size: big ? 1.5 : 1.1, spray: big ? 0.12 : 0.07, lift: rand(1.2, 2.2),
        onArrive: () => {
          pulseAgent(a.agent, big ? 1.2 : 0.8);
          if (Math.random() < 0.65) fire(to, from, color, { power: 0.5, dur: 0.9, size: 0.7 });
        },
      });
      if (coin) {
        const s = sats.get(coin);
        if (s) {
          s.heat = 1;
          shock(s.org, 1);
        }
      }
      if (a.agent === "spotter" && a.level === "trade") {
        coreHeat = 1;
        shock(core, 1.3);
        ringU.uPulse.value = 1.5;
        fireScan(opts.colors.spotter);
        setTimeout(() => fireScan(opts.colors.tracker), 260);
      }
    }, delay);
  }

  /* ---------- labels ---------- */
  const center = document.createElement("div");
  center.className = "lab3 core";
  labels.appendChild(center);
  const proj = new THREE.Vector3();
  let W = 1, H = 1;
  function place(el, v, dy = 0) {
    proj.copy(v).applyMatrix4(world.matrixWorld).project(camera);
    if (proj.z > 1) return (el.style.display = "none");
    el.style.display = "";
    el.style.transform = `translate(-50%, 0) translate(${((proj.x + 1) / 2) * W}px, ${((1 - proj.y) / 2) * H + dy}px)`;
  }

  /* ---------- sizing ---------- */
  function resize() {
    W = container.clientWidth || 1;
    H = container.clientHeight || 1;
    renderer.setSize(W, H, false);
    composer.setSize(W, H);
    camera.aspect = W / H;
    // Keep the whole ring in view on narrow screens.
    const dist = 17.5 * Math.pow(Math.max(1, 1.45 / camera.aspect), 0.62);
    camera.position.setLength(dist);
    camera.updateProjectionMatrix();
  }
  new ResizeObserver(resize).observe(container);
  resize();

  /* ---------- frame loop ---------- */
  const clock = new THREE.Clock();
  let scanClock = 0, chatterClock = 0;
  function stepOrganism(org, dt, energy) {
    org.u.uEnergy.value = energy;
    if (org.shockT < 1) {
      org.shockT += dt / 0.9;
      org.u.uShock.value = org.shockT * 1.2;
      org.u.uShockAmp.value *= Math.exp(-dt * 1.2);
    } else {
      org.u.uShock.value = -1;
    }
  }

  function frame() {
    requestAnimationFrame(frame);
    const dt = Math.min(clock.getDelta(), 0.05);
    const now = clock.elapsedTime;
    const speed = reduced ? 0.25 : 1;
    const focal = camera.position.length();
    for (const u of materials) {
      u.uTime.value = now;
      u.uPixelRatio.value = pixelRatio;
      u.uFocal.value = focal;
    }
    ringU.uTime.value = now;
    ringU.uPulse.value *= Math.exp(-dt * 1.5);

    for (let i = 0; i < N; i++) {
      pA[i] += pW[i] * dt * speed;
      const r = pR[i] * (1 + Math.sin(now * 0.6 + i) * 0.012);
      D.position[i * 3] = Math.cos(pA[i]) * r;
      D.position[i * 3 + 1] = pY[i];
      D.position[i * 3 + 2] = Math.sin(pA[i]) * r;
    }
    disk.attributes.position.needsUpdate = true;
    for (const t of threads) t.rotation.y += t.userData.w * dt * speed;
    sweep.rotation.z -= dt * 0.9 * speed;
    for (const b of billboards) b.quaternion.copy(camera.quaternion);

    coreHeat *= Math.exp(-dt * 1.8);
    core.points.rotation.y += dt * 0.12 * speed;
    core.points.rotation.x = Math.sin(now * 0.2) * 0.15;
    stepOrganism(core, dt, coreHeat);
    coreHaze.material.uniforms.uGlow.value = 0.55 + coreHeat * 0.6;
    coreHaze.material.uniforms.uTime.value = now;
    coreWire.rotation.y += dt * 0.2;
    coreWire.rotation.x += dt * 0.07;

    for (const [, a] of agents) {
      a.heat *= Math.exp(-dt * 1.6);
      a.g.position.set(a.home.x, Math.sin(now * 1.3 + a.phase) * 0.22, a.home.z);
      a.org.points.rotation.y += dt * (0.35 + a.heat * 2.5) * speed;
      a.org.points.rotation.z += dt * 0.15 * speed;
      stepOrganism(a.org, dt, Math.min(1, a.heat));
      a.hz.material.uniforms.uGlow.value = 0.3 + Math.min(1, a.heat) * 0.35;
      a.hz.material.uniforms.uTime.value = now + a.phase;
      a.el.classList.toggle("hot", a.heat > 0.25);
      place(a.el, a.g.position, 24);
    }

    for (const l of links) {
      posOf(l.a, A);
      posOf(l.b, B);
      C.copy(A).add(B).multiplyScalar(0.5).multiplyScalar(0.85);
      C.y += 0.9;
      const arr = l.line.geometry.attributes.position.array;
      for (let k = 0; k <= 32; k++) {
        const t = k / 32, u = 1 - t;
        arr[k * 3] = u * u * A.x + 2 * u * t * C.x + t * t * B.x;
        arr[k * 3 + 1] = u * u * A.y + 2 * u * t * C.y + t * t * B.y;
        arr[k * 3 + 2] = u * u * A.z + 2 * u * t * C.z + t * t * B.z;
      }
      l.line.geometry.attributes.position.needsUpdate = true;
      l.heat *= Math.exp(-dt * 1.4);
      l.line.material.opacity = 0.05 + l.heat * 0.35;
    }

    for (const [coin, s] of sats) {
      s.a += s.w * dt * speed;
      const grow = Math.min(1, Math.max(0.02, (performance.now() - s.born) / 900));
      s.heat *= Math.exp(-dt * 1.5);
      satWorldPos(s, tmp);
      s.body.position.set(Math.cos(s.a) * s.r, 0, Math.sin(s.a) * s.r);
      s.body.scale.setScalar(s.size * 2.2 * grow);
      s.org.points.rotation.y += dt * 0.5;
      stepOrganism(s.org, dt, s.heat);
      s.hz.material.uniforms.uGlow.value = 0.35 + s.heat * 0.5;
      const tp = s.tether.geometry.attributes.position.array;
      tp[3] = tmp.x; tp[4] = tmp.y; tp[5] = tmp.z;
      s.tether.geometry.attributes.position.needsUpdate = true;
      s.tether.material.opacity = 0.1 + s.heat * 0.45;
      place(s.el, tmp, -16 - s.size * 40);
      s.el.dataset.coin = coin;
    }
    place(center, P.set(0, -2.1, 0));

    scanClock += dt;
    if (scanClock > CONFIG.scanEverySec) {
      scanClock = 0;
      fireScan();
    }
    for (const s of scans) {
      if (s.userData.t >= 1) continue;
      s.userData.t += dt / 2.6;
      const t = s.userData.t;
      s.scale.setScalar(1 + t * (CONFIG.ringRadius + 2));
      s.material.opacity = (1 - t) * 0.4;
    }

    chatterHeat *= Math.exp(-dt * 0.5);
    chatterClock += dt * (CONFIG.ambientBeamsPerSec + chatterHeat * 14) * (reduced ? 0.2 : 1);
    while (chatterClock > 1) {
      chatterClock -= 1;
      ambient();
    }

    // Comets: head bright and white-hot, tail tapering with a spray of sparks.
    for (let bi = 0; bi < POOL; bi++) {
      const b = beams[bi];
      const base = bi * T;
      if (!b.active) {
        for (let k = 0; k < T; k++) BA.aSize[base + k] = 0;
        continue;
      }
      b.t += dt / b.dur;
      const head = Math.min(1, b.t), tail = Math.max(0, head - b.trail);
      const fade = b.t > 1 ? Math.max(0, 1 - (b.t - 1) / 0.3) : 1;
      for (let k = 0; k < T; k++) {
        const f = k / (T - 1);
        curvePoint(b, tail + (head - tail) * f, P);
        const spray = b.spray * (1 - f) * (1 + Math.sin(now * 7 + k));
        const j = jitter[k];
        const i3 = (base + k) * 3;
        BA.position[i3] = P.x + j.x * spray;
        BA.position[i3 + 1] = P.y + j.y * spray;
        BA.position[i3 + 2] = P.z + j.z * spray;
        const inten = Math.pow(f, 1.5) * b.power * fade;
        const hot = Math.pow(f, 8) * 0.75;
        BA.aColor[i3] = (b.color.r + (WHITE.r - b.color.r) * hot) * inten;
        BA.aColor[i3 + 1] = (b.color.g + (WHITE.g - b.color.g) * hot) * inten;
        BA.aColor[i3 + 2] = (b.color.b + (WHITE.b - b.color.b) * hot) * inten;
        BA.aSize[base + k] = CONFIG.beamSize * b.size * (0.35 + 1.4 * f * f + (k === T - 1 ? 1.6 : 0)) * (b.t > 1 ? fade : 1);
      }
      if (b.t >= 1 && b.onArrive) {
        const cb = b.onArrive;
        b.onArrive = null;
        cb();
      }
      if (b.t > 1.3) b.active = false;
    }
    beamGeo.attributes.position.needsUpdate = true;
    beamGeo.attributes.aColor.needsUpdate = true;
    beamGeo.attributes.aSize.needsUpdate = true;

    controls.update();
    composer.render();
  }
  requestAnimationFrame(frame);

  return {
    sync(state, fresh) {
      const maxV = Math.max(1, ...state.target.positions.map((p) => p.value));
      const keep = new Set();
      for (const p of state.target.positions) {
        keep.add(p.coin);
        let s = sats.get(p.coin);
        const long = p.size > 0;
        if (s && s.long !== long) {
          dropSat(p.coin);
          s = null;
        }
        if (!s) {
          s = satellite(p);
          sats.set(p.coin, s);
        }
        s.long = long;
        s.size = 0.1 + 0.2 * Math.sqrt(p.value / maxV);
        s.el.innerHTML = `<b style="color:${long ? opts.long : opts.short}">${p.coin} ${long ? "▲" : "▼"}</b>`;
      }
      for (const k of [...sats.keys()]) if (!keep.has(k)) dropSat(k);
      for (const a of state.agents) {
        const ag = agents.get(a.id);
        if (ag) ag.count.textContent = a.count + " Schritte";
      }
      const short = state.wallet ? state.wallet.slice(0, 6) + "…" + state.wallet.slice(-4) : "–";
      center.innerHTML = `<span>WALLET</span><b>${short}</b>`;
      fresh.forEach((a, i) => onActivity(a, i * 140));
    },
  };

  function dropSat(coin) {
    const s = sats.get(coin);
    if (!s) return;
    world.remove(s.pivot, s.tether);
    const i = materials.indexOf(s.org.u);
    if (i >= 0) materials.splice(i, 1);
    billboards.splice(billboards.indexOf(s.hz), 1);
    for (const o of [s.org.points, s.hz, s.orbitLine, s.tether]) {
      o.geometry.dispose();
      o.material.dispose();
    }
    s.el.remove();
    sats.delete(coin);
  }
}
