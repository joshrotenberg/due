// Build-time authoring adapter. This never starts Due or a provider.
import fs from "node:fs";
import {compileStringEx} from "squint-cljs";

const here = new URL("./", import.meta.url);
const dist = new URL("./dist/", here);
const javascriptExample = new URL("../quickjs/", here);
fs.mkdirSync(dist, {recursive: true});

for (const name of ["main", "pipeline"]) {
  const source = fs.readFileSync(new URL(`${name}.cljs`, here), "utf8");
  const compiled = compileStringEx(source, {
    "source-map": {file: `${name}.js`, source: `${name}.cljs`}
  });
  // The spike admits only exact local/* imports. Squint's JS API does not
  // apply a plain-object import-maps option for its standard-library specifier.
  const header = "import * as squint_core from 'squint-cljs/core.js';";
  if (!compiled.javascript.startsWith(header + "\n")) {
    throw new Error("Squint core import changed; inspect the pinned compiler output");
  }
  const javascript = 'import * as squint_core from "local/squint-core";' +
    compiled.javascript.slice(header.length);
  fs.writeFileSync(new URL(`${name}.js`, dist),
    javascript + `\n//# sourceMappingURL=${name}.js.map\n`);
  fs.writeFileSync(new URL(`${name}.js.map`, dist), compiled["source-map-json"]);
}

fs.copyFileSync(new URL(import.meta.resolve("squint-cljs/src/squint/core.js")),
  new URL("squint-core.js", dist));
fs.copyFileSync(new URL("js-review.js", here), new URL("js-review.js", dist));
for (const file of ["input.json", "input.schema.json", "output.schema.json", "answer.schema.json"]) {
  fs.copyFileSync(new URL(file, javascriptExample), new URL(file, dist));
}
const manifest = JSON.parse(fs.readFileSync(new URL("workflow.json", javascriptExample), "utf8"));
manifest.modules = {
  "local/pipeline": "pipeline.js",
  "local/js-review": "js-review.js",
  "local/squint-core": "squint-core.js"
};
fs.writeFileSync(new URL("workflow.json", dist), JSON.stringify(manifest, null, 2) + "\n");
console.log("Compiled Squint sources and captured its runtime in dist/. No agents started.");
