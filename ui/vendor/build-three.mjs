/* Rebuild `ui/vendor/three.js` — the window's one WebGL renderer, vendored as
 * one file. Two views draw with it: the relations tab's 3D view and the
 * knowledge graph's GL painter.
 *
 *   node ui/vendor/build-three.mjs
 *
 * Same road as `build-cm6.mjs`, for the same reasons: the window has no
 * bundler and no network at runtime, `index.html` is served as it sits on
 * disk, and the CSP is `default-src 'self'`. three.js ships as ESM on npm, so
 * the npm half happens HERE, once, offline, in a scratch directory OUTSIDE the
 * checkout — and what lands in the repository is a single IIFE that a plain
 * `<script src>` can take, plus this script and the licence. No worker and no
 * `blob:` URL is in the output, so the CSP is untouched.
 *
 * Why three.js at all: the 3D view moved from canvas 2D to WebGL2 on
 * 2026-09-27, by the person's decision after the approved prototype
 * (docs/design/board-3d-preview/index.html). See the header of
 * `ui/shell-board-orbit.js` for the rest of that argument. The knowledge
 * graph's hand-written WebGL2 painter moved onto the same renderer the same
 * day, at the person's request, so the window keeps one GL renderer rather
 * than two (header of `ui/shell-knowledge-3d.js`).
 *
 * What comes out attaches to `window.THREE`, and holds only what ENTRY names
 * below: this file is the whole definition of that surface, so a symbol either
 * view wants and cannot see is added here and the script re-run.
 *
 * Measured before committing anything: the byte count of the output is printed
 * at the end, and the ledger quotes it.
 */

import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync, readFileSync, statSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const VENDOR = dirname(fileURLToPath(import.meta.url));
const OUT = join(VENDOR, "three.js");
/* Outside the checkout on purpose. A scratch tree inside `ui/` would be
 * picked up by the dev server, the Rust gates that read `ui/**`, and git. */
const WORK = join(tmpdir(), "zerocode-three-build");

/* Pinned, so re-running this on another machine produces the same renderer.
 * r160 is the revision the approved prototype was drawn and reviewed on — its
 * shaders, bloom and colour management were judged against exactly this
 * three.js, so the product takes the same one rather than a newer revision
 * that looks different in ways nobody reviewed. Bumping it is a deliberate
 * edit to this list, never a `^` drifting underneath us. */
const DEPS = {
  three: "0.160.1",
  esbuild: "0.28.2",
};

/* The entry. Written out rather than kept as a file beside this one because
 * it is only ever read by the esbuild run below — a second file in
 * `ui/vendor/` would look like something the window loads, and it is not.
 *
 * The first block is every `T.` the prototype's script reads. The second is
 * what the product needs beyond a one-scene prototype: instancing so draw
 * calls stay flat as agents arrive (InstancedMesh and the transform types that
 * compose an instance matrix, or merged geometry), buffers rewritten every
 * frame (DynamicDrawUsage), fitting the camera to content (Box3), and the
 * side/tone-mapping/colour-space constants a custom bloom pass switches
 * between. */
const ENTRY = String.raw`
import {
  ACESFilmicToneMapping, AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture,
  CapsuleGeometry, CatmullRomCurve3, CircleGeometry, Color, CylinderGeometry, DirectionalLight,
  DoubleSide, EdgesGeometry, ExtrudeGeometry, Group, HalfFloatType, HemisphereLight,
  LineBasicMaterial, LineSegments, Mesh, MeshBasicMaterial, MeshStandardMaterial, NoColorSpace,
  OrthographicCamera, PCFSoftShadowMap, PlaneGeometry, Points, Raycaster, RepeatWrapping,
  RingGeometry, SRGBColorSpace, Scene, ShaderMaterial, ShadowMaterial, Shape, ShapeGeometry,
  SphereGeometry, TorusGeometry, TubeGeometry, Vector2, Vector3, Vector4, WebGLRenderTarget,
  WebGLRenderer,
} from "three"
import {
  InstancedMesh, InstancedBufferAttribute, Object3D, Matrix4, Quaternion, Euler, Box3,
  MathUtils, DynamicDrawUsage, FrontSide, BackSide, NoToneMapping, LinearSRGBColorSpace,
  REVISION,
} from "three"
/* The knowledge graph's painter (ui/shell-knowledge-3d.js), moved onto the same
 * renderer on 2026-09-27: raw GLSL 300 es programs over instanced quads, and a
 * float texture that holds every point's seat. */
import {
  InstancedBufferGeometry, RawShaderMaterial, DataTexture, FloatType, RGBAFormat, NearestFilter,
  ClampToEdgeWrapping, GLSL3, NormalBlending,
} from "three"
/* The knowledge graph's universe view (ui/shell-knowledge-universe.js, t-12443):
 * the approved prototype v4 drew through an orthographic camera whose
 * projection it overwrote with a perspective one, because this bundle had no
 * perspective camera. The product takes the real one instead of the detour. */
import { PerspectiveCamera } from "three"
import {mergeGeometries} from "three/addons/utils/BufferGeometryUtils.js"

window.THREE = {
  ACESFilmicToneMapping, AdditiveBlending, BufferAttribute, BufferGeometry, CanvasTexture,
  CapsuleGeometry, CatmullRomCurve3, CircleGeometry, Color, CylinderGeometry, DirectionalLight,
  DoubleSide, EdgesGeometry, ExtrudeGeometry, Group, HalfFloatType, HemisphereLight,
  LineBasicMaterial, LineSegments, Mesh, MeshBasicMaterial, MeshStandardMaterial, NoColorSpace,
  OrthographicCamera, PCFSoftShadowMap, PlaneGeometry, Points, Raycaster, RepeatWrapping,
  RingGeometry, SRGBColorSpace, Scene, ShaderMaterial, ShadowMaterial, Shape, ShapeGeometry,
  SphereGeometry, TorusGeometry, TubeGeometry, Vector2, Vector3, Vector4, WebGLRenderTarget,
  WebGLRenderer,
  InstancedMesh, InstancedBufferAttribute, Object3D, Matrix4, Quaternion, Euler, Box3,
  MathUtils, DynamicDrawUsage, FrontSide, BackSide, NoToneMapping, LinearSRGBColorSpace,
  REVISION,
  mergeGeometries,
  InstancedBufferGeometry, RawShaderMaterial, DataTexture, FloatType, RGBAFormat, NearestFilter,
  ClampToEdgeWrapping, GLSL3, NormalBlending,
  PerspectiveCamera,
}
`;

const run = (command, args, cwd) => {
  const done = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (done.status !== 0) {
    console.error(`\n${command} ${args.join(" ")} failed (${done.status})`);
    process.exit(1);
  }
};

rmSync(WORK, { recursive: true, force: true });
mkdirSync(WORK, { recursive: true });
writeFileSync(
  join(WORK, "package.json"),
  `${JSON.stringify({ name: "zerocode-three-build", private: true, type: "module", dependencies: DEPS }, null, 2)}\n`,
);
writeFileSync(join(WORK, "entry.mjs"), ENTRY);

console.log(`installing into ${WORK} …`);
run("npm", ["install", "--no-audit", "--no-fund", "--silent"], WORK);

console.log("bundling …");
run(
  join(WORK, "node_modules", ".bin", "esbuild"),
  [
    "entry.mjs",
    "--bundle",
    "--format=iife",
    "--minify",
    "--legal-comments=none",
    "--target=safari15",
    // three.js warns through `console.warn` — "multiple instances" at load,
    // deprecations and misuse later. None of it is for the person using this
    // window, and the window's console is read as signal, so the calls are
    // declared side-effect free and the minifier drops them. `console.error`
    // (a shader that did not compile) stays.
    "--pure:console.warn",
    `--outfile=${OUT}`,
  ],
  WORK,
);

/* What the CSP and the console rule above promise, checked on the bytes. */
const built = readFileSync(OUT, "utf8");
for (const smell of ["console.warn", "new Worker(", "blob:", "importScripts("]) {
  if (built.includes(smell)) {
    console.error(`\n${OUT} contains \`${smell}\``);
    process.exit(1);
  }
}

/* The licence travels with the code. three.js has no dependencies, so the
 * bundle is exactly one package; its name, version and licence are read off
 * the install rather than typed. */
const pkg = JSON.parse(readFileSync(join(WORK, "node_modules", "three", "package.json"), "utf8"));
if (pkg.license !== "MIT") {
  console.error(`\n${pkg.name}@${pkg.version} is ${pkg.license}, not MIT, so this notice cannot cover it`);
  process.exit(1);
}

writeFileSync(
  join(VENDOR, "three-LICENSE"),
  `ui/vendor/three.js is a build of three.js. It contains, in part, the\n` +
    `following package — MIT, under the notice reproduced below. Regenerate\n` +
    `both this file and the bundle with \`node ui/vendor/build-three.mjs\`.\n\n` +
    `  ${pkg.name}@${pkg.version} (${pkg.license})\n\n` +
    `${readFileSync(join(WORK, "node_modules", "three", "LICENSE"), "utf8")}`,
);

console.log(`\n${OUT}  ${statSync(OUT).size} bytes`);
console.log(`three-LICENSE  ${pkg.name}@${pkg.version}, MIT`);
