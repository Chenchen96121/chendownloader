import assert from "node:assert/strict";
import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import {
  auditPeDependencies,
  auditPortableTree,
  readTrustedManifest,
  stagePortableTree,
  validateVendorInputs,
} from "./portable-package.mjs";

const execFile = promisify(execFileCallback);
const SCRIPT_DIRECTORY = path.dirname(fileURLToPath(import.meta.url));

const TARGET = "x86_64-pc-windows-msvc";
const TOOL_VERSIONS = {
  "yt-dlp": "2026.08.19",
  deno: "2.9.7",
  "gallery-dl": "1.32.15",
  ffmpeg: "9.0.2-chendownloader-d7643a0fe3",
  ffprobe: "9.0.2-chendownloader-d7643a0fe3",
};
const LICENSE_FILES = [
  "DEPENDENCY-NOTICES.txt",
  "deno-MIT.txt",
  "ffmpeg-GPL-3.0.txt",
  "ffmpeg-build-configuration.txt",
  "gallery-dl-GPL-2.0.txt",
  "yt-dlp-THIRD-PARTY-LICENSES.txt",
  "yt-dlp-UNLICENSE.txt",
];
const GALLERY_DL_SOURCE_ARCHIVE = "gallery-dl-1.32.15.tar.gz";
const GALLERY_DL_SOURCE_CONTENT = "gallery-dl 1.32.15 source archive fixture\n";
const GALLERY_DL_SOURCE_RECORD = `Component: gallery-dl
Version: 1.32.15
Upstream-Project: https://codeberg.org/mikf/gallery-dl
Upstream-Release: https://codeberg.org/mikf/gallery-dl/releases/tag/v1.32.15
Upstream-Source: https://files.pythonhosted.org/packages/source/g/gallery-dl/gallery_dl-1.32.15.tar.gz
Retrieved-At: 2026-10-10T10:21:24Z
License: GPL-2.0-only
Modified: no
Executable-SHA256: \${GALLERY_DL_EXECUTABLE_SHA256}
Source-Archive-SHA256: ${digest(GALLERY_DL_SOURCE_CONTENT)}
`;

function digest(content) {
  return createHash("sha256").update(content).digest("hex");
}

function toolFilename(name) {
  return `${name}-${TARGET}.exe`;
}

async function writeText(filePath, content) {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, content, "utf8");
}

async function makeFixture() {
  const fixtureRoot = await mkdtemp(path.join(os.tmpdir(), "portable-package-test-"));
  const repositoryRoot = path.join(fixtureRoot, "source");
  const vendorRoot = path.join(fixtureRoot, "vendor");
  const outputRoot = path.join(fixtureRoot, "output", "chendownloader-0.2.0-windows-x64");
  const appPath = path.join(fixtureRoot, "build", "chendownloader.exe");
  const galleryDlSourceArchive = path.join(fixtureRoot, "source-input", GALLERY_DL_SOURCE_ARCHIVE);

  const tools = Object.entries(TOOL_VERSIONS).map(([name, version]) => {
    const content = `fixture executable for ${name}\n`;
    return { name, version, sha256: digest(content), content };
  });
  const manifest = {
    schemaVersion: 1,
    target: TARGET,
    tools: tools.map(({ content: _content, ...tool }) => tool),
  };

  await writeText(
    path.join(repositoryRoot, "src-tauri", "resources", "managed-tools", "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  await writeText(path.join(repositoryRoot, "package.json"), '{"version":"0.2.0"}\n');
  await writeText(path.join(repositoryRoot, "src-tauri", "tauri.conf.json"), '{"version":"0.2.0"}\n');
  await writeText(path.join(repositoryRoot, "src-tauri", "Cargo.toml"), '[package]\nversion = "0.2.0"\n');
  await writeText(path.join(repositoryRoot, "packaging", "README.txt"), "Portable release instructions.\n");
  await writeText(path.join(repositoryRoot, "THIRD_PARTY_NOTICES.md"), "# Third-party notices\n");
  await writeText(path.join(repositoryRoot, "LICENSE"), "MIT License\nCopyright (c) 2026 Chenchen96121\n");
  for (const filename of LICENSE_FILES) {
    await writeText(path.join(repositoryRoot, "packaging", "licenses", filename), `${filename}\n`);
  }
  await writeText(
    path.join(repositoryRoot, "packaging", "licenses", "gallery-dl-source-record.txt"),
    GALLERY_DL_SOURCE_RECORD,
  );
  for (const tool of tools) {
    await writeText(path.join(vendorRoot, toolFilename(tool.name)), tool.content);
  }
  await writeText(appPath, "fixture application executable\n");
  await writeText(galleryDlSourceArchive, GALLERY_DL_SOURCE_CONTENT);

  const probeRunner = async ({ tool }) => {
    if (tool.name === "yt-dlp" || tool.name === "gallery-dl") return `${tool.version}\n`;
    if (tool.name === "deno") return `deno ${tool.version}\n`;
    return `${tool.name} version ${tool.version}\n`;
  };
  const peInspector = async () => ["KERNEL32.dll", "USER32.dll"];

  return {
    fixtureRoot,
    repositoryRoot,
    vendorRoot,
    outputRoot,
    appPath,
    galleryDlSourceArchive,
    manifest,
    probeRunner,
    peInspector,
    async cleanup() {
      await rm(fixtureRoot, { recursive: true, force: true });
    },
  };
}

async function withFixture(run) {
  const fixture = await makeFixture();
  try {
    await run(fixture);
  } finally {
    await fixture.cleanup();
  }
}

test("readTrustedManifest accepts the pinned manifest shape", async () => {
  await withFixture(async ({ repositoryRoot, manifest }) => {
    assert.deepEqual(await readTrustedManifest({ repositoryRoot }), manifest);
  });
});

test("portable release preserves the application license and detects its modification", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    const bundled = path.join(fixture.outputRoot, "LICENSE");
    assert.equal(await readFile(bundled, "utf8"), await readFile(path.join(fixture.repositoryRoot, "LICENSE"), "utf8"));
    await writeText(bundled, "changed license\n");
    await assert.rejects(() => auditPortableTree({
      repositoryRoot: fixture.repositoryRoot,
      portableRoot: fixture.outputRoot,
      peInspector: fixture.peInspector,
    }), /LICENSE.*digest|static release material/i);
  });
});

test("staging requires the application license", async () => {
  await withFixture(async (fixture) => {
    await rm(path.join(fixture.repositoryRoot, "LICENSE"));
    await assert.rejects(() => stagePortableTree(fixture), /LICENSE|regular file/i);
  });
});

test("stagePortableTree produces only the deterministic portable allowlist", async () => {
  await withFixture(async (fixture) => {
    const result = await stagePortableTree(fixture);
    assert.equal(result.toolCount, 5);
    assert.equal(result.fileCount, 20);

    const audit = await auditPortableTree({
      repositoryRoot: fixture.repositoryRoot,
      portableRoot: fixture.outputRoot,
      sourceRoots: [fixture.repositoryRoot, fixture.vendorRoot, fixture.fixtureRoot],
      peInspector: fixture.peInspector,
    });
    assert.equal(audit.fileCount, 20);
    assert.deepEqual(audit.relativePaths, [
      "chendownloader.exe",
      "README.txt",
      "LICENSE",
      "THIRD_PARTY_NOTICES.md",
      "licenses/DEPENDENCY-NOTICES.txt",
      "licenses/deno-MIT.txt",
      "licenses/ffmpeg-GPL-3.0.txt",
      "licenses/ffmpeg-build-configuration.txt",
      "licenses/gallery-dl-GPL-2.0.txt",
      "licenses/gallery-dl-source-record.txt",
      `licenses/sources/${GALLERY_DL_SOURCE_ARCHIVE}`,
      "licenses/yt-dlp-THIRD-PARTY-LICENSES.txt",
      "licenses/yt-dlp-UNLICENSE.txt",
      "release.json",
      `resources/managed-tools/deno-${TARGET}.exe`,
      `resources/managed-tools/ffmpeg-${TARGET}.exe`,
      `resources/managed-tools/ffprobe-${TARGET}.exe`,
      `resources/managed-tools/gallery-dl-${TARGET}.exe`,
      `resources/managed-tools/manifest.json`,
      `resources/managed-tools/yt-dlp-${TARGET}.exe`,
    ].sort());

    const galleryTool = fixture.manifest.tools.find((tool) => tool.name === "gallery-dl");
    const sourceRecord = await readFile(
      path.join(fixture.outputRoot, "licenses", "gallery-dl-source-record.txt"),
      "utf8",
    );
    assert.match(sourceRecord, /^Upstream-Project: https:\/\/codeberg\.org\/mikf\/gallery-dl$/m);
    assert.match(
      sourceRecord,
      /^Upstream-Release: https:\/\/codeberg\.org\/mikf\/gallery-dl\/releases\/tag\/v1\.32\.15$/m,
    );
    assert.match(sourceRecord, /^Retrieved-At: \d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/m);
    assert.match(sourceRecord, new RegExp(`Executable-SHA256: ${galleryTool.sha256}`));
    assert.match(
      sourceRecord,
      new RegExp(`Source-Archive-SHA256: ${digest(GALLERY_DL_SOURCE_CONTENT)}`),
    );

    const release = JSON.parse(await readFile(path.join(fixture.outputRoot, "release.json"), "utf8"));
    assert.deepEqual(release, {
      application: "chendownloader",
      applicationSha256: digest("fixture application executable\n"),
      format: "portable-windows-x64",
      schemaVersion: 1,
      version: "0.2.0",
    });
  });
});

test("stagePortableTree rejects a malformed gallery-dl retrieval timestamp", async () => {
  await withFixture(async (fixture) => {
    const recordPath = path.join(
      fixture.repositoryRoot,
      "packaging",
      "licenses",
      "gallery-dl-source-record.txt",
    );
    const record = await readFile(recordPath, "utf8");
    await writeText(
      recordPath,
      record.replace("Retrieved-At: 2026-10-10T10:21:24Z", "Retrieved-At: 2026-02-30T10:21:24Z"),
    );
    await assert.rejects(() => stagePortableTree(fixture), /retrieval timestamp/i);
  });
});

test("validateVendorInputs rejects missing, extra, and renamed tools", async (t) => {
  await t.test("missing", async () => {
    await withFixture(async (fixture) => {
      await rm(path.join(fixture.vendorRoot, toolFilename("gallery-dl")));
      await assert.rejects(() => validateVendorInputs(fixture), /exactly|missing|gallery-dl/i);
    });
  });
  await t.test("extra", async () => {
    await withFixture(async (fixture) => {
      await writeText(path.join(fixture.vendorRoot, "extra.exe"), "extra\n");
      await assert.rejects(() => validateVendorInputs(fixture), /unexpected|extra/i);
    });
  });
  await t.test("renamed", async () => {
    await withFixture(async (fixture) => {
      const original = path.join(fixture.vendorRoot, toolFilename("yt-dlp"));
      const content = await readFile(original);
      await rm(original);
      await writeFile(path.join(fixture.vendorRoot, "yt-dlp.exe"), content);
      await assert.rejects(() => validateVendorInputs(fixture), /unexpected|missing|yt-dlp/i);
    });
  });
});

test("validateVendorInputs rejects a wrong digest or probe version", async (t) => {
  await t.test("digest", async () => {
    await withFixture(async (fixture) => {
      await writeText(path.join(fixture.vendorRoot, toolFilename("ffmpeg")), "tampered\n");
      await assert.rejects(() => validateVendorInputs(fixture), /sha-?256|digest/i);
    });
  });
  await t.test("version", async () => {
    await withFixture(async (fixture) => {
      await assert.rejects(
        () => validateVendorInputs({
          ...fixture,
          probeRunner: async ({ tool }) => tool.name === "gallery-dl" ? "1.32.14\n" : fixture.probeRunner({ tool }),
        }),
        /gallery-dl.*version|version.*gallery-dl/i,
      );
    });
  });
});

test("stagePortableTree requires the gallery-dl license and explicit source archive", async (t) => {
  await t.test("license", async () => {
    await withFixture(async (fixture) => {
      await rm(path.join(fixture.repositoryRoot, "packaging", "licenses", "gallery-dl-GPL-2.0.txt"));
      await assert.rejects(() => stagePortableTree(fixture), /gallery-dl-GPL-2\.0|license|regular file/i);
    });
  });
  await t.test("source archive", async () => {
    await withFixture(async (fixture) => {
      await rm(fixture.galleryDlSourceArchive);
      await assert.rejects(() => stagePortableTree(fixture), /gallery-dl.*source|source.*archive|regular file/i);
    });
  });
  await t.test("source archive digest", async () => {
    await withFixture(async (fixture) => {
      await writeText(fixture.galleryDlSourceArchive, "different source archive\n");
      await assert.rejects(() => stagePortableTree(fixture), /source.*sha-?256|source.*digest/i);
    });
  });
});

test("auditPortableTree verifies the bundled gallery-dl source digest", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    await writeText(
      path.join(fixture.outputRoot, "licenses", "sources", GALLERY_DL_SOURCE_ARCHIVE),
      "tampered source archive\n",
    );
    await assert.rejects(
      () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
      /gallery-dl.*source|source.*sha-?256|source.*digest/i,
    );
  });
});

test("stagePortableTree rejects a gallery-dl executable with undeclared PE imports", async () => {
  await withFixture(async (fixture) => {
    await assert.rejects(
      () => stagePortableTree({
        ...fixture,
        peInspector: async (executablePath) => path.basename(executablePath).startsWith("gallery-dl-")
          ? ["KERNEL32.dll", "python313.dll"]
          : ["KERNEL32.dll", "USER32.dll"],
      }),
      /gallery-dl.*python313|undeclared.*python313|python313.*undeclared/i,
    );
  });
});

test("auditPortableTree allows only the exact gallery-dl source archive", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    await writeText(path.join(fixture.outputRoot, "licenses", "sources", "other.tar.gz"), "other\n");
    await assert.rejects(
      () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
      /other\.tar\.gz|forbidden|unexpected|allowlist/i,
    );
  });
});

test("auditPortableTree rejects missing release notices", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    await rm(path.join(fixture.outputRoot, "THIRD_PARTY_NOTICES.md"));
    await assert.rejects(
      () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
      /missing|allowlist|THIRD_PARTY_NOTICES/i,
    );
  });
});

test("auditPortableTree rejects an unexpected empty directory", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    await mkdir(path.join(fixture.outputRoot, "unexpected-empty"));
    await assert.rejects(() => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }), /directory|allowlist/i);
  });
});

test("auditPortableTree requires the visible manifest to match the embedded bytes", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    const visible = path.join(fixture.outputRoot, "resources", "managed-tools", "manifest.json");
    await writeFile(visible, `${await readFile(visible, "utf8")} `);
    await assert.rejects(() => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }), /manifest.*bytes|byte.*manifest/i);
  });
});

test("auditPortableTree rejects altered notices and application bytes", async (t) => {
  await t.test("altered notice", async () => {
    await withFixture(async (fixture) => {
      await stagePortableTree(fixture);
      await writeText(path.join(fixture.outputRoot, "README.txt"), "replacement notice\n");
      await assert.rejects(
        () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
        /README\.txt|static release material|digest/i,
      );
    });
  });
  await t.test("altered application", async () => {
    await withFixture(async (fixture) => {
      await stagePortableTree(fixture);
      await writeText(path.join(fixture.outputRoot, "chendownloader.exe"), "replacement executable\n");
      await assert.rejects(
        () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
        /application|chendownloader|digest/i,
      );
    });
  });
});

test("auditPortableTree rejects forbidden private, media, catalog, test, and nested archive files", async (t) => {
  const forbidden = [
    "cookies.txt",
    "session.cookies",
    "history.sqlite",
    "captured.mp4",
    "download.part",
    "catalog.json",
    "vitest-results.json",
    "nested.zip",
  ];
  for (const filename of forbidden) {
    await t.test(filename, async () => {
      await withFixture(async (fixture) => {
        await stagePortableTree(fixture);
        await writeText(path.join(fixture.outputRoot, filename), "forbidden\n");
        await assert.rejects(
          () => auditPortableTree({ ...fixture, portableRoot: fixture.outputRoot }),
          /forbidden|unexpected|allowlist|archive/i,
        );
      });
    });
  }
});

test("auditPortableTree rejects absolute build-path leakage", async () => {
  await withFixture(async (fixture) => {
    await stagePortableTree(fixture);
    const leakedContent = `accidental source path: ${fixture.repositoryRoot}\n`;
    await writeText(path.join(fixture.repositoryRoot, "packaging", "README.txt"), leakedContent);
    await writeText(path.join(fixture.outputRoot, "README.txt"), leakedContent);
    await assert.rejects(
      () => auditPortableTree({
        ...fixture,
        portableRoot: fixture.outputRoot,
        sourceRoots: [fixture.repositoryRoot, fixture.vendorRoot],
      }),
      /absolute path|leak/i,
    );
  });
});

test("auditPeDependencies rejects undeclared imports", async () => {
  await withFixture(async (fixture) => {
    await assert.rejects(
      () => auditPeDependencies({
        executablePaths: [fixture.appPath],
        peInspector: async () => ["KERNEL32.dll", "surprise-runtime.dll"],
      }),
      /surprise-runtime\.dll|undeclared/i,
    );
  });
});

test("auditPeDependencies accepts the pinned Deno Windows system imports", async () => {
  await withFixture(async (fixture) => {
    const result = await auditPeDependencies({
      executablePaths: [fixture.appPath],
      peInspector: async () => [
        "KERNEL32.dll",
        "dbghelp.dll",
        "opengl32.dll",
        "combase.dll",
        "rstrtmgr.dll",
        "psapi.dll",
      ],
    });
    assert.equal(result.executableCount, 1);
    assert.equal(result.importCount, 6);
  });
});

test("auditPeDependencies accepts the pinned FFmpeg Windows system imports", async () => {
  await withFixture(async (fixture) => {
    const result = await auditPeDependencies({
      executablePaths: [fixture.appPath],
      peInspector: async () => ["AVRT.dll", "MSIMG32.dll", "ncrypt.dll", "AVICAP32.dll"],
    });
    assert.equal(result.importCount, 4);
  });
});

test("stagePortableTree refuses an existing destination and destinations inside inputs", async (t) => {
  await t.test("existing", async () => {
    await withFixture(async (fixture) => {
      await mkdir(fixture.outputRoot, { recursive: true });
      await assert.rejects(() => stagePortableTree(fixture), /already exists|new destination/i);
    });
  });
  await t.test("inside repository", async () => {
    await withFixture(async (fixture) => {
      await assert.rejects(
        () => stagePortableTree({ ...fixture, outputRoot: path.join(fixture.repositoryRoot, "portable") }),
        /inside|overlap/i,
      );
    });
  });
  await t.test("inside vendor", async () => {
    await withFixture(async (fixture) => {
      await assert.rejects(
        () => stagePortableTree({ ...fixture, outputRoot: path.join(fixture.vendorRoot, "portable") }),
        /inside|overlap/i,
      );
    });
  });
  await t.test("allows only the dedicated ignored release area inside the repository", async () => {
    await withFixture(async (fixture) => {
      const outputRoot = path.join(fixture.repositoryRoot, ".release", "chendownloader-0.2.0-windows-x64");
      const result = await stagePortableTree({ ...fixture, outputRoot });
      assert.equal(result.rootPath, outputRoot);
    });
  });
});

test("stagePortableTree rejects unsynchronized application versions", async () => {
  await withFixture(async (fixture) => {
    await writeText(path.join(fixture.repositoryRoot, "src-tauri", "tauri.conf.json"), '{"version":"0.2.1"}\n');
    await assert.rejects(() => stagePortableTree(fixture), /versions are not synchronized/i);
  });
});

test("build-portable.ps1 requires an explicit gallery-dl source archive", { skip: process.platform !== "win32" }, async () => {
  const missingVendor = path.join(os.tmpdir(), `missing-portable-vendor-${process.pid}-${Date.now()}`);
  let failure;
  try {
    await execFile("powershell.exe", [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      path.join(SCRIPT_DIRECTORY, "build-portable.ps1"),
      "-VendorRoot",
      missingVendor,
    ], { encoding: "utf8", timeout: 10_000, windowsHide: true });
  } catch (error) {
    failure = error;
  }
  assert.ok(failure, "a deliberately missing vendor root must be rejected");
  const diagnostics = `${failure.stdout ?? ""}\n${failure.stderr ?? ""}`;
  assert.doesNotMatch(diagnostics, /Join-Path.*empty string|PSScriptRoot/i);
  assert.match(diagnostics, /GalleryDlSourceArchive|gallery-dl source archive/i);
});

test("build-portable.ps1 binds its default output before validating the vendor root", { skip: process.platform !== "win32" }, async () => {
  await withFixture(async (fixture) => {
    const missingVendor = path.join(os.tmpdir(), `missing-portable-vendor-${process.pid}-${Date.now()}`);
    let failure;
    try {
      await execFile("powershell.exe", [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        path.join(SCRIPT_DIRECTORY, "build-portable.ps1"),
        "-VendorRoot",
        missingVendor,
        "-GalleryDlSourceArchive",
        fixture.galleryDlSourceArchive,
      ], { encoding: "utf8", timeout: 10_000, windowsHide: true });
    } catch (error) {
      failure = error;
    }
    assert.ok(failure, "a deliberately missing vendor root must be rejected");
    const diagnostics = `${failure.stdout ?? ""}\n${failure.stderr ?? ""}`;
    assert.doesNotMatch(diagnostics, /Join-Path.*empty string|PSScriptRoot/i);
    assert.match(diagnostics, /VendorRoot|cannot find path|does not exist/i);
  });
});

async function runPortableOutputValidation(fixture, outputRoot) {
  const scriptPath = path.join(SCRIPT_DIRECTORY, "build-portable.ps1");
  const script = await readFile(scriptPath, "utf8");
  const marker = "$applicationVersion =";
  const boundary = script.indexOf(marker);
  assert.ok(boundary > 0, "validation boundary must remain before build side effects");
  const repositoryRoot = path.dirname(SCRIPT_DIRECTORY).replaceAll("'", "''");
  const validationOnly = `${script.slice(0, boundary)
    .replace(
      '$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))',
      `$repositoryRoot = '${repositoryRoot}'`,
    )}\nreturn\n`;
  const validationScript = path.join(fixture.fixtureRoot, `validate-output-${Date.now()}.ps1`);
  await writeFile(validationScript, validationOnly, "utf8");
  return execFile("powershell.exe", [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    validationScript,
    "-VendorRoot",
    fixture.vendorRoot,
    "-GalleryDlSourceArchive",
    fixture.galleryDlSourceArchive,
    "-OutputRoot",
    outputRoot,
  ], { encoding: "utf8", timeout: 10_000, windowsHide: true });
}

test("build-portable.ps1 rejects dangerous or non-fresh custom output roots before cleanup", { skip: process.platform !== "win32" }, async (t) => {
  await withFixture(async (fixture) => {
    const nonEmpty = await mkdtemp(path.join(os.homedir(), `lmd-output-policy-test-${process.pid}-`));
    try {
      await writeFile(path.join(nonEmpty, "keep.txt"), "must survive\n", "utf8");

      for (const [name, outputRoot, diagnostic] of [
        ["non-empty directory", nonEmpty, /fresh|empty/i],
        ["filesystem root", path.parse(fixture.fixtureRoot).root, /filesystem root|drive root/i],
        ["user profile", os.homedir(), /protected|profile/i],
      ]) {
        await t.test(name, async () => {
          await assert.rejects(
            () => runPortableOutputValidation(fixture, outputRoot),
            diagnostic,
          );
        });
      }
      assert.equal(await readFile(path.join(nonEmpty, "keep.txt"), "utf8"), "must survive\n");
    } finally {
      await rm(nonEmpty, { recursive: true, force: true });
    }
  });
});

test("build-portable.ps1 requests reproducible statically linked Windows output", async () => {
  const script = await readFile(path.join(SCRIPT_DIRECTORY, "build-portable.ps1"), "utf8");
  assert.match(script, /target-feature=\+crt-static/);
  assert.match(script, /link-arg=\/Brepro/i);
  assert.match(script, /portable-package\.mjs["',\s]+hash["',\s]+--file/i);
  assert.doesNotMatch(script, /Get-FileHash/i);
});

test("hash CLI returns a lowercase SHA-256 digest without PowerShell module discovery", async () => {
  await withFixture(async ({ fixtureRoot }) => {
    const target = path.join(fixtureRoot, "portable.zip");
    const content = "portable archive fixture\n";
    await writeText(target, content);

    const { stdout } = await execFile(process.execPath, [
      path.join(SCRIPT_DIRECTORY, "portable-package.mjs"),
      "hash",
      "--file",
      target,
    ], { encoding: "utf8", windowsHide: true });

    assert.deepEqual(JSON.parse(stdout), { sha256: digest(content) });
  });
});
