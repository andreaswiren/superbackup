// SVG -> PNG, using the same renderer the application does.
//
// `crates/app/src/tray/icons.rs` rasterises with `resvg`; `@resvg/resvg-js` is
// that library with a JS wrapper. Rendering the reference bitmaps through a
// different engine would let the checked-in previews disagree with what the
// running program draws, which is the exact failure this whole directory
// exists to prevent.
//
// Usage: node render.js <jobs.json>
//   jobs.json: [{ "svg": "<path>", "png": "<path>", "size": 256 }, ...]

const fs = require("fs");
const { Resvg } = require("@resvg/resvg-js");

const jobs = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));

for (const job of jobs) {
  const svg = fs.readFileSync(job.svg, "utf8");
  const resvg = new Resvg(svg, {
    fitTo: { mode: "width", value: job.size },
    background: job.background || undefined,
    shapeRendering: 2, // geometricPrecision: no hinting games at 16 px
    imageRendering: 0,
  });
  fs.writeFileSync(job.png, resvg.render().asPng());
}

console.log(`rendered ${jobs.length}`);
