import { createHash } from "node:crypto";
import { execFile as execFileCallback } from "node:child_process";
import {
  copyFile,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  realpath,
  rm,
  stat,
  utimes,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFile = promisify(execFileCallback);
const APPLICATION_NAME = "chendownloader";
const PORTABLE_EXECUTABLE = "chendownloader.exe";
const REQUIRED_TOOL_NAMES = ["yt-dlp", "deno", "gallery-dl", "ffmpeg", "ffprobe"];
const LICENSE_FILES = [
  "DEPENDENCY-NOTICES.txt",
  "deno-MIT.txt",
  "ffmpeg-GPL-3.0.txt",
  "ffmpeg-build-configuration.txt",
  "gallery-dl-GPL-2.0.txt",
  "yt-dlp-THIRD-PARTY-LICENSES.txt",
  "yt-dlp-UNLICENSE.txt",
];
const GALLERY_DL_VERSION = "1.32.15";
const GALLERY_DL_SOURCE_ARCHIVE = `gallery-dl-${GALLERY_DL_VERSION}.tar.gz`;
const GALLERY_DL_SOURCE_RECORD = "gallery-dl-source-record.txt";
const GALLERY_DL_EXECUTABLE_SHA_PLACEHOLDER = "${GALLERY_DL_EXECUTABLE_SHA256}";
const GALLERY_DL_PROJECT_URL = "https://codeberg.org/mikf/gallery-dl";
const GALLERY_DL_RELEASE_URL = "https://codeberg.org/mikf/gallery-dl/releases/tag/v1.32.15";
const GALLERY_DL_SOURCE_URL = "https://files.pythonhosted.org/packages/source/g/gallery-dl/gallery_dl-1.32.15.tar.gz";
const FIXED_TIMESTAMP = new Date("2000-01-01T00:00:00.000Z");
const TEXT_EXTENSIONS = new Set([".json", ".md", ".txt"]);
const SYSTEM_DLLS = new Set([
  "advapi32.dll",
  "avicap32.dll",
  "avrt.dll",
  "bcrypt.dll",
  "bcryptprimitives.dll",
  "cfgmgr32.dll",
  "combase.dll",
  "comctl32.dll",
  "comdlg32.dll",
  "crypt32.dll",
  "d3d11.dll",
  "dbghelp.dll",
  "dnsapi.dll",
  "dwmapi.dll",
  "dxgi.dll",
  "gdi32.dll",
  "imm32.dll",
  "iphlpapi.dll",
  "kernel32.dll",
  "msvcrt.dll",
  "msimg32.dll",
  "ncrypt.dll",
  "ntdll.dll",
  "ole32.dll",
  "oleaut32.dll",
  "opengl32.dll",
  "powrprof.dll",
  "psapi.dll",
  "propsys.dll",
  "rpcrt4.dll",
  "rstrtmgr.dll",
  "secur32.dll",
  "setupapi.dll",
  "shell32.dll",
  "shlwapi.dll",
  "ucrtbase.dll",
  "user32.dll",
  "userenv.dll",
  "usp10.dll",
  "uxtheme.dll",
  "version.dll",
  "webview2loader.dll",
  "winhttp.dll",
  "wininet.dll",
  "winmm.dll",
  "wtsapi32.dll",
  "ws2_32.dll",
]);

function requireAbsolute(label, value) {
  if (typeof value !== "string" || !path.isAbsolute(value)) {
    throw new Error(`${label} must be an absolute path`);
  }
  return path.resolve(value);
}

function isWithin(parent, candidate) {
  const relative = path.relative(parent, candidate);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== "..");
}

function pathsOverlap(left, right) {
  return isWithin(left, right) || isWithin(right, left);
}

function toolFilename(tool, target) {
  return `${tool}-${target}.exe`;
}

function manifestPath(repositoryRoot) {
  return path.join(repositoryRoot, "src-tauri", "resources", "managed-tools", "manifest.json");
}

function assertManifest(manifest) {
  if (manifest?.schemaVersion !== 1 || manifest?.target !== "x86_64-pc-windows-msvc") {
    throw new Error("Trusted tool manifest has an unsupported schema or target");
  }
  if (!Array.isArray(manifest.tools) || manifest.tools.length !== REQUIRED_TOOL_NAMES.length) {
    throw new Error("Trusted tool manifest must declare exactly five tools");
  }
  const names = manifest.tools.map((tool) => tool?.name);
  if (names.some((name, index) => name !== REQUIRED_TOOL_NAMES[index])) {
    throw new Error("Trusted tool manifest names or ordering do not match the release policy");
  }
  for (const tool of manifest.tools) {
    if (typeof tool.version !== "string" || tool.version.length === 0) {
      throw new Error(`Trusted tool manifest is missing the ${tool.name} version`);
    }
    if (typeof tool.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(tool.sha256)) {
      throw new Error(`Trusted tool manifest has an invalid ${tool.name} SHA-256`);
    }
  }
  const galleryDl = manifest.tools.find((tool) => tool.name === "gallery-dl");
  if (galleryDl?.version !== GALLERY_DL_VERSION) {
    throw new Error(`Trusted tool manifest must pin gallery-dl ${GALLERY_DL_VERSION}`);
  }
  return manifest;
}

function parseGalleryDlSourceRecord(text, { allowExecutablePlaceholder = false } = {}) {
  const lines = String(text).replace(/\r\n/g, "\n").trimEnd().split("\n");
  const expectedKeys = [
    "Component",
    "Version",
    "Upstream-Project",
    "Upstream-Release",
    "Upstream-Source",
    "Retrieved-At",
    "License",
    "Modified",
    "Executable-SHA256",
    "Source-Archive-SHA256",
  ];
  if (lines.length !== expectedKeys.length) throw new Error("gallery-dl source record has an invalid shape");
  const values = {};
  for (let index = 0; index < expectedKeys.length; index += 1) {
    const prefix = `${expectedKeys[index]}: `;
    if (!lines[index].startsWith(prefix)) throw new Error("gallery-dl source record has an invalid shape");
    values[expectedKeys[index]] = lines[index].slice(prefix.length);
  }
  if (
    values.Component !== "gallery-dl"
    || values.Version !== GALLERY_DL_VERSION
    || values["Upstream-Project"] !== GALLERY_DL_PROJECT_URL
    || values["Upstream-Release"] !== GALLERY_DL_RELEASE_URL
    || values["Upstream-Source"] !== GALLERY_DL_SOURCE_URL
    || values.License !== "GPL-2.0-only"
    || values.Modified !== "no"
  ) {
    throw new Error("gallery-dl source record does not match the release policy");
  }
  const retrievedAt = values["Retrieved-At"];
  const parsedRetrievedAt = Date.parse(retrievedAt);
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(retrievedAt)
    || Number.isNaN(parsedRetrievedAt)
    || new Date(parsedRetrievedAt).toISOString() !== retrievedAt.replace(/Z$/, ".000Z")
  ) {
    throw new Error("gallery-dl source record has an invalid UTC retrieval timestamp");
  }
  const executableSha = values["Executable-SHA256"];
  if (
    !/^[a-f0-9]{64}$/.test(executableSha)
    && !(allowExecutablePlaceholder && executableSha === GALLERY_DL_EXECUTABLE_SHA_PLACEHOLDER)
  ) {
    throw new Error("gallery-dl source record has an invalid executable SHA-256");
  }
  const sourceSha = values["Source-Archive-SHA256"];
  if (!/^[a-f0-9]{64}$/.test(sourceSha)) {
    throw new Error("gallery-dl source record has an invalid source archive SHA-256");
  }
  return { executableSha, sourceSha };
}

async function readGalleryDlSourceTemplate(repositoryRoot) {
  const source = path.join(repositoryRoot, "packaging", "licenses", GALLERY_DL_SOURCE_RECORD);
  await assertRegularFileWithin(repositoryRoot, source, "gallery-dl source record");
  const template = await readFile(source, "utf8");
  const record = parseGalleryDlSourceRecord(template, { allowExecutablePlaceholder: true });
  if (record.executableSha !== GALLERY_DL_EXECUTABLE_SHA_PLACEHOLDER) {
    throw new Error("gallery-dl source record template must contain the executable SHA-256 placeholder");
  }
  return { source, template: template.replace(/\r\n/g, "\n"), record };
}

async function assertRegularFile(filePath, label = filePath) {
  const entry = await lstat(filePath).catch(() => null);
  if (!entry?.isFile() || entry.isSymbolicLink()) {
    throw new Error(`${label} must be a regular file and may not be a symlink`);
  }
  return filePath;
}

async function assertNoSymlinkComponents(root, candidate, label) {
  if (!isWithin(root, candidate)) throw new Error(`${label} is outside its trusted root`);
  const relative = path.relative(root, candidate);
  let current = root;
  for (const component of ["", ...relative.split(path.sep).filter(Boolean)]) {
    if (component) current = path.join(current, component);
    const entry = await lstat(current).catch((error) => {
      if (error?.code === "ENOENT") return null;
      throw error;
    });
    if (!entry) break;
    if (entry.isSymbolicLink()) throw new Error(`${label} may not traverse a symlink or junction`);
  }
}

async function assertRegularFileWithin(root, filePath, label = filePath) {
  await assertNoSymlinkComponents(root, filePath, label);
  await assertRegularFile(filePath, label);
  const [canonicalRoot, canonicalFile] = await Promise.all([realpath(root), realpath(filePath)]);
  if (!isWithin(canonicalRoot, canonicalFile)) {
    throw new Error(`${label} resolves outside its trusted root`);
  }
  return filePath;
}

export async function sha256File(filePath) {
  const handle = await open(filePath, "r");
  try {
    const hash = createHash("sha256");
    for await (const chunk of handle.createReadStream()) hash.update(chunk);
    return hash.digest("hex");
  } finally {
    await handle.close();
  }
}

async function defaultProbeRunner({ tool, executablePath }) {
  const args = tool.name === "ffmpeg" || tool.name === "ffprobe" ? ["-version"] : ["--version"];
  const { stdout, stderr } = await execFile(executablePath, args, {
    encoding: "utf8",
    maxBuffer: 16 * 1024,
    shell: false,
    timeout: 10_000,
    windowsHide: true,
  });
  return `${stdout ?? ""}\n${stderr ?? ""}`.trim();
}

function assertProbeVersion(tool, output) {
  const text = String(output).trim();
  let detected;
  if (tool.name === "yt-dlp" || tool.name === "gallery-dl") detected = text.split(/\r?\n/, 1)[0]?.trim();
  if (tool.name === "deno") detected = text.match(/^deno\s+(\S+)/m)?.[1];
  if (tool.name === "ffmpeg" || tool.name === "ffprobe") {
    detected = text.match(new RegExp(`^${tool.name} version\\s+(\\S+)`, "m"))?.[1];
  }
  if (detected !== tool.version) {
    throw new Error(`${tool.name} version probe returned ${detected ?? "an unrecognized value"}; expected ${tool.version}`);
  }
}

export async function readTrustedManifest({ repositoryRoot }) {
  const root = requireAbsolute("repositoryRoot", repositoryRoot);
  const source = manifestPath(root);
  await assertRegularFileWithin(root, source, "trusted tool manifest");
  const raw = await readFile(source, "utf8");
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error(`Trusted tool manifest is not valid JSON: ${error.message}`);
  }
  return assertManifest(parsed);
}

async function readApplicationVersion(repositoryRoot) {
  const packagePath = path.join(repositoryRoot, "package.json");
  const tauriPath = path.join(repositoryRoot, "src-tauri", "tauri.conf.json");
  const cargoPath = path.join(repositoryRoot, "src-tauri", "Cargo.toml");
  for (const source of [packagePath, tauriPath, cargoPath]) {
    await assertRegularFileWithin(repositoryRoot, source, path.relative(repositoryRoot, source));
  }
  const packageVersion = JSON.parse(await readFile(packagePath, "utf8"))?.version;
  const tauriVersion = JSON.parse(await readFile(tauriPath, "utf8"))?.version;
  const cargoVersion = (await readFile(cargoPath, "utf8")).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (
    typeof packageVersion !== "string"
    || packageVersion !== tauriVersion
    || packageVersion !== cargoVersion
  ) {
    throw new Error(`Application versions are not synchronized: package=${packageVersion}, tauri=${tauriVersion}, cargo=${cargoVersion}`);
  }
  return packageVersion;
}

export async function validateVendorInputs({ manifest, vendorRoot, probeRunner = defaultProbeRunner }) {
  assertManifest(manifest);
  const root = requireAbsolute("vendorRoot", vendorRoot);
  const rootEntry = await lstat(root).catch(() => null);
  if (!rootEntry?.isDirectory() || rootEntry.isSymbolicLink()) {
    throw new Error("vendorRoot must be a real directory, not a symlink");
  }

  const expected = manifest.tools.map((tool) => toolFilename(tool.name, manifest.target)).sort();
  const entries = await readdir(root, { withFileTypes: true });
  const actual = entries.map((entry) => entry.name).sort();
  if (actual.length !== expected.length || actual.some((name, index) => name !== expected[index])) {
    const missing = expected.filter((name) => !actual.includes(name));
    const unexpected = actual.filter((name) => !expected.includes(name));
    throw new Error(`Vendor directory must contain exactly the trusted tools; missing: ${missing.join(", ") || "none"}; unexpected: ${unexpected.join(", ") || "none"}`);
  }

  const validated = [];
  for (const tool of manifest.tools) {
    const filename = toolFilename(tool.name, manifest.target);
    const executablePath = path.join(root, filename);
    await assertRegularFile(executablePath, filename);
    const resolved = await realpath(executablePath);
    if (!isWithin(await realpath(root), resolved)) {
      throw new Error(`${filename} resolves outside vendorRoot`);
    }
    const actualDigest = await sha256File(executablePath);
    if (actualDigest !== tool.sha256) {
      throw new Error(`${filename} SHA-256 digest does not match the trusted manifest`);
    }
    const output = await probeRunner({ tool: { ...tool }, executablePath, args: tool.name.startsWith("ff") ? ["-version"] : ["--version"] });
    assertProbeVersion(tool, output);
    validated.push({ ...tool, filename, executablePath });
  }
  return { toolCount: validated.length, tools: validated };
}

const TIKTOK_ARTIFACT="tiktok-core-x86_64-pc-windows-msvc.exe";
const TIKTOK_NOTICES="tiktok-core-NOTICES.txt";
function requiresTikTok(version){const [major,minor]=version.split(".").map(Number);return major>1||(major===1&&minor>=1);}
function tikTokManifestPath(root){return path.join(root,"src-tauri/resources/tiktok-core/manifest.json");}
export async function validateTikTokCore({repositoryRoot,executablePath,probeRunner=defaultProbeRunner}){
 const root=requireAbsolute("repositoryRoot",repositoryRoot);
 const executable=requireAbsolute("tiktokCorePath",executablePath);
 const manifestFile=tikTokManifestPath(root);
 await assertRegularFileWithin(root,manifestFile);
 const manifest=JSON.parse(await readFile(manifestFile,"utf8"));
 const fields=["schemaVersion","protocolVersion","target","version","artifact","sha256"];
 if(Object.keys(manifest).length!==fields.length||fields.some(key=>!Object.hasOwn(manifest,key))||manifest.schemaVersion!==1||manifest.protocolVersion!==1||manifest.target!=="x86_64-pc-windows-msvc"||manifest.version!=="1.1.0"||manifest.artifact!==TIKTOK_ARTIFACT||!/^[a-f0-9]{64}$/u.test(manifest.sha256))throw new Error("Invalid TikTok trusted manifest");
 await assertNoSymlinkComponents(path.parse(executable).root,executable,"TikTok component");
 await assertRegularFile(executable);
 if(await sha256File(executable)!==manifest.sha256)throw new Error("TikTok component digest differs from trusted manifest");
 await assertRegularFileWithin(root,path.join(root,"packaging/licenses",TIKTOK_NOTICES));
 const output=await probeRunner({tool:{name:"tiktok-core",version:manifest.version},executablePath:executable,args:["--version"]});
 if(output.trim()!=="chendownloader-tiktok-core 1.1.0 protocol=1")throw new Error("TikTok component identity differs from trusted manifest");
 return {manifest,executablePath:executable};
}

function expectedPortablePaths(manifest,tiktok=false) {
  return [
    "LICENSE",
    PORTABLE_EXECUTABLE,
    "README.txt",
    "THIRD_PARTY_NOTICES.md",
    ...LICENSE_FILES.map((filename) => `licenses/${filename}`),
    `licenses/${GALLERY_DL_SOURCE_RECORD}`,
    `licenses/sources/${GALLERY_DL_SOURCE_ARCHIVE}`,
    "release.json",
    "resources/managed-tools/manifest.json",
    ...manifest.tools.map((tool) => `resources/managed-tools/${toolFilename(tool.name, manifest.target)}`),
    ...(tiktok?[`resources/platform-tools/${TIKTOK_ARTIFACT}`,"resources/platform-tools/manifest.json",`licenses/${TIKTOK_NOTICES}`]:[]),
  ].sort();
}

async function walkPortableTree(root) {
  const files = [];
  async function walk(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Portable tree contains a forbidden symlink: ${absolute}`);
      if (entry.isDirectory()) {
        const relativeDirectory = path.relative(root, absolute).split(path.sep).join("/");
        if (!["licenses", "licenses/sources", "resources", "resources/managed-tools","resources/platform-tools"].includes(relativeDirectory)) {
          throw new Error(`Portable directory is outside the release allowlist: ${relativeDirectory}`);
        }
        await walk(absolute);
      } else if (entry.isFile()) {
        files.push(path.relative(root, absolute).split(path.sep).join("/"));
      } else {
        throw new Error(`Portable tree contains a non-regular entry: ${absolute}`);
      }
    }
  }
  await walk(root);
  return files.sort();
}

function assertNoForbiddenPath(relativePath) {
  if (relativePath === `licenses/sources/${GALLERY_DL_SOURCE_ARCHIVE}`) return;
  const lower = relativePath.toLowerCase();
  const base = path.posix.basename(lower);
  const extension = path.posix.extname(lower);
  const forbiddenExtensions = new Set([
    ".7z", ".avi", ".cookie", ".cookies", ".db", ".gz", ".log", ".m4a", ".mkv", ".mov",
    ".mp3", ".mp4", ".msi", ".msix", ".part", ".sqlite", ".sqlite3", ".tar", ".webm",
    ".ytdl", ".zip",
  ]);
  if (
    forbiddenExtensions.has(extension)
    || base === "cookies.txt"
    || base.startsWith("catalog.")
    || /(^|[-_.])(test|vitest|jest)([-_.]|$)/.test(base)
  ) {
    throw new Error(`Portable tree contains a forbidden release path: ${relativePath}`);
  }
}

function readAsciiZ(buffer, offset, label) {
  if (!Number.isInteger(offset) || offset < 0 || offset >= buffer.length) {
    throw new Error(`Invalid ${label} string offset in PE image`);
  }
  const end = buffer.indexOf(0, offset);
  if (end < 0 || end - offset > 512) throw new Error(`Unterminated ${label} string in PE image`);
  return buffer.toString("ascii", offset, end);
}

function parsePeImports(buffer, executablePath) {
  const ensure = (offset, length, label) => {
    if (!Number.isInteger(offset) || offset < 0 || offset + length > buffer.length) {
      throw new Error(`${path.basename(executablePath)} has a truncated ${label}`);
    }
  };
  ensure(0, 64, "DOS header");
  if (buffer.readUInt16LE(0) !== 0x5a4d) throw new Error(`${path.basename(executablePath)} is not a PE executable`);
  const peOffset = buffer.readUInt32LE(0x3c);
  ensure(peOffset, 24, "PE header");
  if (buffer.readUInt32LE(peOffset) !== 0x00004550) throw new Error(`${path.basename(executablePath)} has an invalid PE signature`);

  const sectionCount = buffer.readUInt16LE(peOffset + 6);
  const optionalSize = buffer.readUInt16LE(peOffset + 20);
  const optionalOffset = peOffset + 24;
  ensure(optionalOffset, optionalSize, "optional header");
  const magic = buffer.readUInt16LE(optionalOffset);
  if (magic !== 0x10b && magic !== 0x20b) throw new Error(`${path.basename(executablePath)} has an unsupported PE optional header`);
  const imageBase = magic === 0x20b
    ? Number(buffer.readBigUInt64LE(optionalOffset + 24))
    : buffer.readUInt32LE(optionalOffset + 28);
  const dataDirectoryOffset = optionalOffset + (magic === 0x20b ? 112 : 96);
  const dataDirectoryCount = buffer.readUInt32LE(optionalOffset + (magic === 0x20b ? 108 : 92));
  const sectionOffset = optionalOffset + optionalSize;
  ensure(sectionOffset, sectionCount * 40, "section table");

  const sections = [];
  for (let index = 0; index < sectionCount; index += 1) {
    const offset = sectionOffset + index * 40;
    sections.push({
      virtualSize: buffer.readUInt32LE(offset + 8),
      virtualAddress: buffer.readUInt32LE(offset + 12),
      rawSize: buffer.readUInt32LE(offset + 16),
      rawOffset: buffer.readUInt32LE(offset + 20),
    });
  }
  const rvaToOffset = (rva, label) => {
    const section = sections.find((candidate) => {
      const size = Math.max(candidate.virtualSize, candidate.rawSize);
      return rva >= candidate.virtualAddress && rva < candidate.virtualAddress + size;
    });
    if (!section) throw new Error(`${path.basename(executablePath)} has an invalid ${label} RVA`);
    const offset = section.rawOffset + rva - section.virtualAddress;
    ensure(offset, 1, label);
    return offset;
  };

  const imports = [];
  const readDirectory = (directoryIndex, descriptorSize, nameOffset) => {
    if (directoryIndex >= dataDirectoryCount) return;
    const directoryEntry = dataDirectoryOffset + directoryIndex * 8;
    if (directoryEntry + 8 > optionalOffset + optionalSize) return;
    const tableRva = buffer.readUInt32LE(directoryEntry);
    const tableSize = buffer.readUInt32LE(directoryEntry + 4);
    if (tableRva === 0 || tableSize === 0) return;
    const tableOffset = rvaToOffset(tableRva, "import directory");
    const descriptorLimit = Math.min(Math.ceil(tableSize / descriptorSize) + 1, 4096);
    for (let index = 0; index < descriptorLimit; index += 1) {
      const descriptorOffset = tableOffset + index * descriptorSize;
      ensure(descriptorOffset, descriptorSize, "import descriptor");
      let allZero = true;
      for (let byte = 0; byte < descriptorSize; byte += 1) {
        if (buffer[descriptorOffset + byte] !== 0) {
          allZero = false;
          break;
        }
      }
      if (allZero) return;
      const storedName = buffer.readUInt32LE(descriptorOffset + nameOffset);
      const usesRvas = directoryIndex !== 13 || (buffer.readUInt32LE(descriptorOffset) & 1) === 1;
      const nameRva = usesRvas ? storedName : storedName - imageBase;
      if (nameRva === 0) throw new Error(`${path.basename(executablePath)} has an import descriptor without a DLL name`);
      imports.push(readAsciiZ(buffer, rvaToOffset(nameRva, "DLL name"), "DLL name"));
    }
    throw new Error(`${path.basename(executablePath)} has an unterminated import directory`);
  };

  readDirectory(1, 20, 12);
  readDirectory(13, 32, 4);
  return [...new Set(imports)];
}

async function defaultPeInspector(executablePath) {
  return parsePeImports(await readFile(executablePath), executablePath);
}

function isAllowedDll(name) {
  const lower = name.toLowerCase();
  return SYSTEM_DLLS.has(lower) || lower.startsWith("api-ms-win-") || lower.startsWith("ext-ms-win-");
}

export async function auditPeDependencies({ executablePaths, peInspector = defaultPeInspector }) {
  if (!Array.isArray(executablePaths) || executablePaths.length === 0) {
    throw new Error("PE audit requires at least one executable");
  }
  let importCount = 0;
  for (const candidate of executablePaths) {
    const executablePath = requireAbsolute("executablePath", candidate);
    await assertRegularFile(executablePath);
    const imports = await peInspector(executablePath);
    if (!Array.isArray(imports) || imports.length === 0) {
      throw new Error(`PE inspector returned no dependencies for ${path.basename(executablePath)}`);
    }
    const dynamicRuntime = imports.filter((imported) => /^(vcruntime|msvcp|concrt)\d*[^/\\]*\.dll$/i.test(imported));
    if (dynamicRuntime.length > 0) {
      throw new Error(`${path.basename(executablePath)} depends on forbidden dynamic Visual C++ runtime DLLs: ${dynamicRuntime.join(", ")}`);
    }
    const undeclared = imports.filter((imported) => typeof imported !== "string" || !isAllowedDll(imported));
    if (undeclared.length > 0) {
      throw new Error(`${path.basename(executablePath)} has undeclared PE dependencies: ${undeclared.join(", ")}`);
    }
    for (const imported of imports) {
      importCount += 1;
    }
  }
  return { executableCount: executablePaths.length, importCount };
}

async function assertNoAbsolutePathLeak(portableRoot, relativePaths, sourceRoots) {
  const needles = sourceRoots
    .filter(Boolean)
    .map((root) => requireAbsolute("sourceRoot", root))
    .flatMap((root) => [root, root.split(path.sep).join("/"), root.split(path.sep).join("\\")])
    .map((root) => root.toLowerCase());
  for (const relativePath of relativePaths) {
    if (!TEXT_EXTENSIONS.has(path.posix.extname(relativePath).toLowerCase())) continue;
    const content = (await readFile(path.join(portableRoot, ...relativePath.split("/")), "utf8")).toLowerCase();
    const leaked = needles.find((needle) => needle.length > 2 && content.includes(needle));
    if (leaked) throw new Error(`${relativePath} contains an absolute path leak`);
  }
}

export async function auditPortableTree({ repositoryRoot, portableRoot, sourceRoots = [], peInspector = defaultPeInspector,probeRunner=defaultProbeRunner }) {
  const repository = requireAbsolute("repositoryRoot", repositoryRoot);
  const root = requireAbsolute("portableRoot", portableRoot);
  const rootEntry = await lstat(root).catch(() => null);
  if (!rootEntry?.isDirectory() || rootEntry.isSymbolicLink()) {
    throw new Error("portableRoot must be a real directory, not a symlink");
  }
  const manifest = await readTrustedManifest({ repositoryRoot: repository });
  const applicationVersion = await readApplicationVersion(repository);
  const relativePaths = await walkPortableTree(root);
  for (const relativePath of relativePaths) assertNoForbiddenPath(relativePath);

  const tiktok=requiresTikTok(applicationVersion);
  const expected = expectedPortablePaths(manifest,tiktok);
  if (relativePaths.length !== expected.length || relativePaths.some((name, index) => name !== expected[index])) {
    const missing = expected.filter((name) => !relativePaths.includes(name));
    const unexpected = relativePaths.filter((name) => !expected.includes(name));
    throw new Error(`Portable tree does not match the release allowlist; missing: ${missing.join(", ") || "none"}; unexpected: ${unexpected.join(", ") || "none"}`);
  }

  const bundledManifestPath = path.join(root, "resources", "managed-tools", "manifest.json");
  if (!(await readFile(bundledManifestPath)).equals(await readFile(manifestPath(repository)))) {
    throw new Error("Bundled manifest bytes differ from the embedded source manifest");
  }
  const bundledManifest = assertManifest(JSON.parse(await readFile(bundledManifestPath, "utf8")));
  if (JSON.stringify(bundledManifest) !== JSON.stringify(manifest)) {
    throw new Error("Bundled tool manifest differs from the trusted source manifest");
  }
  for (const tool of manifest.tools) {
    const executablePath = path.join(root, "resources", "managed-tools", toolFilename(tool.name, manifest.target));
    if (await sha256File(executablePath) !== tool.sha256) {
      throw new Error(`Bundled ${tool.name} SHA-256 digest does not match the trusted manifest`);
    }
  }

  const galleryDl = manifest.tools.find((tool) => tool.name === "gallery-dl");
  const sourceTemplate = await readGalleryDlSourceTemplate(repository);
  const expectedSourceRecord = sourceTemplate.template.replace(
    GALLERY_DL_EXECUTABLE_SHA_PLACEHOLDER,
    galleryDl.sha256,
  );
  const bundledSourceRecordPath = path.join(root, "licenses", GALLERY_DL_SOURCE_RECORD);
  const bundledSourceRecord = (await readFile(bundledSourceRecordPath, "utf8")).replace(/\r\n/g, "\n");
  if (bundledSourceRecord !== expectedSourceRecord) {
    throw new Error("Bundled gallery-dl source record differs from the trusted release record");
  }
  const sourceRecord = parseGalleryDlSourceRecord(bundledSourceRecord);
  if (sourceRecord.executableSha !== galleryDl.sha256) {
    throw new Error("Bundled gallery-dl executable SHA-256 differs from the source record");
  }
  const bundledSourceArchive = path.join(root, "licenses", "sources", GALLERY_DL_SOURCE_ARCHIVE);
  if (await sha256File(bundledSourceArchive) !== sourceRecord.sourceSha) {
    throw new Error("Bundled gallery-dl source archive SHA-256 differs from the source record");
  }

  const staticMaterials = [
    ...(tiktok?[[tikTokManifestPath(repository),path.join(root,"resources/platform-tools/manifest.json")],[path.join(repository,"packaging/licenses",TIKTOK_NOTICES),path.join(root,"licenses",TIKTOK_NOTICES)]]:[]),
    [path.join(repository, "LICENSE"), path.join(root, "LICENSE")],
    [path.join(repository, "packaging", "README.txt"), path.join(root, "README.txt")],
    [path.join(repository, "THIRD_PARTY_NOTICES.md"), path.join(root, "THIRD_PARTY_NOTICES.md")],
    ...LICENSE_FILES.map((filename) => [
      path.join(repository, "packaging", "licenses", filename),
      path.join(root, "licenses", filename),
    ]),
  ];
  for (const [trustedPath, bundledPath] of staticMaterials) {
    if (await sha256File(trustedPath) !== await sha256File(bundledPath)) {
      throw new Error(`${path.relative(root, bundledPath)} static release material digest differs from the trusted source`);
    }
  }

  const release = JSON.parse(await readFile(path.join(root, "release.json"), "utf8"));
  if (
    release?.schemaVersion !== 1
    || release?.application !== APPLICATION_NAME
    || release?.version !== applicationVersion
    || release?.format !== "portable-windows-x64"
    || typeof release?.applicationSha256 !== "string"
    || !/^[a-f0-9]{64}$/.test(release.applicationSha256)
  ) {
    throw new Error("release.json does not describe the expected portable release");
  }
  if (await sha256File(path.join(root, PORTABLE_EXECUTABLE)) !== release.applicationSha256) {
    throw new Error(`${PORTABLE_EXECUTABLE} digest differs from the release record`);
  }

  await assertNoAbsolutePathLeak(root, relativePaths, sourceRoots);
  const executablePaths = [
    ...(tiktok?[path.join(root,"resources/platform-tools",TIKTOK_ARTIFACT)]:[]),
    path.join(root, PORTABLE_EXECUTABLE),
    ...manifest.tools.map((tool) => path.join(root, "resources", "managed-tools", toolFilename(tool.name, manifest.target))),
  ];
  if(tiktok)await validateTikTokCore({repositoryRoot:repository,executablePath:path.join(root,"resources/platform-tools",TIKTOK_ARTIFACT),probeRunner});
  const pe = await auditPeDependencies({ executablePaths, peInspector });
  return { fileCount: relativePaths.length, relativePaths, ...pe };
}

async function copyReleaseFile(source, destination) {
  await assertRegularFile(source);
  await mkdir(path.dirname(destination), { recursive: true });
  await copyFile(source, destination);
  await utimes(destination, FIXED_TIMESTAMP, FIXED_TIMESTAMP);
}

function isApprovedRepositoryOutput(repositoryRoot, outputRoot) {
  const releaseRoot = path.join(repositoryRoot, ".release");
  return outputRoot !== releaseRoot && isWithin(releaseRoot, outputRoot);
}

export async function stagePortableTree({ repositoryRoot, appPath, vendorRoot, galleryDlSourceArchive, outputRoot,tiktokCorePath, probeRunner = defaultProbeRunner, peInspector = defaultPeInspector }) {
  const repository = requireAbsolute("repositoryRoot", repositoryRoot);
  const application = requireAbsolute("appPath", appPath);
  const vendor = requireAbsolute("vendorRoot", vendorRoot);
  const sourceArchive = requireAbsolute("galleryDlSourceArchive", galleryDlSourceArchive);
  const output = requireAbsolute("outputRoot", outputRoot);
  if (
    (pathsOverlap(output, repository) && !isApprovedRepositoryOutput(repository, output))
    || pathsOverlap(output, vendor)
    || pathsOverlap(output, sourceArchive)
  ) {
    throw new Error("outputRoot may not be inside or overlap repositoryRoot/vendorRoot/galleryDlSourceArchive");
  }
  if (isWithin(repository, sourceArchive)) {
    throw new Error("galleryDlSourceArchive must be an external trusted input");
  }
  if (await stat(output).then(() => true, () => false)) {
    throw new Error("outputRoot already exists; staging requires a new destination");
  }
  const outputBoundary = isApprovedRepositoryOutput(repository, output) ? repository : path.parse(output).root;
  await assertNoSymlinkComponents(outputBoundary, path.dirname(output), "outputRoot");
  await assertRegularFile(application, "application executable");
  await assertNoSymlinkComponents(path.parse(sourceArchive).root, sourceArchive, "gallery-dl source archive");
  await assertRegularFile(sourceArchive, "gallery-dl source archive");
  const manifest = await readTrustedManifest({ repositoryRoot: repository });
  const applicationVersion = await readApplicationVersion(repository);
  const tiktok=requiresTikTok(applicationVersion)?await validateTikTokCore({repositoryRoot:repository,executablePath:tiktokCorePath,probeRunner}):null;
  const validated = await validateVendorInputs({ manifest, vendorRoot: vendor, probeRunner });
  const sourceTemplate = await readGalleryDlSourceTemplate(repository);
  const sourceArchiveSha = await sha256File(sourceArchive);
  if (sourceArchiveSha !== sourceTemplate.record.sourceSha) {
    throw new Error("gallery-dl source archive SHA-256 differs from the trusted source record");
  }

  const requiredSources = [
    path.join(repository, "packaging", "README.txt"),
    path.join(repository, "THIRD_PARTY_NOTICES.md"),
    path.join(repository, "LICENSE"),
    ...LICENSE_FILES.map((filename) => path.join(repository, "packaging", "licenses", filename)),
  ];
  for (const source of requiredSources) await assertRegularFileWithin(repository, source);

  await mkdir(path.dirname(output), { recursive: true });
  await mkdir(output);
  try {
    await copyReleaseFile(application, path.join(output, PORTABLE_EXECUTABLE));
    if(tiktok){
      await copyReleaseFile(tiktok.executablePath,path.join(output,"resources/platform-tools",TIKTOK_ARTIFACT));
      await copyReleaseFile(tikTokManifestPath(repository),path.join(output,"resources/platform-tools/manifest.json"));
      await copyReleaseFile(path.join(repository,"packaging/licenses",TIKTOK_NOTICES),path.join(output,"licenses",TIKTOK_NOTICES));
    }
    await copyReleaseFile(requiredSources[0], path.join(output, "README.txt"));
    await copyReleaseFile(requiredSources[1], path.join(output, "THIRD_PARTY_NOTICES.md"));
    await copyReleaseFile(requiredSources[2], path.join(output, "LICENSE"));
    for (const filename of LICENSE_FILES) {
      await copyReleaseFile(path.join(repository, "packaging", "licenses", filename), path.join(output, "licenses", filename));
    }
    const galleryDl = manifest.tools.find((tool) => tool.name === "gallery-dl");
    const sourceRecord = sourceTemplate.template.replace(
      GALLERY_DL_EXECUTABLE_SHA_PLACEHOLDER,
      galleryDl.sha256,
    );
    const sourceRecordPath = path.join(output, "licenses", GALLERY_DL_SOURCE_RECORD);
    await writeFile(sourceRecordPath, sourceRecord, "utf8");
    await utimes(sourceRecordPath, FIXED_TIMESTAMP, FIXED_TIMESTAMP);
    await copyReleaseFile(
      sourceArchive,
      path.join(output, "licenses", "sources", GALLERY_DL_SOURCE_ARCHIVE),
    );
    await copyReleaseFile(manifestPath(repository), path.join(output, "resources", "managed-tools", "manifest.json"));
    for (const tool of validated.tools) {
      await copyReleaseFile(tool.executablePath, path.join(output, "resources", "managed-tools", tool.filename));
    }
    const release = {
      application: APPLICATION_NAME,
      applicationSha256: await sha256File(application),
      format: "portable-windows-x64",
      schemaVersion: 1,
      version: applicationVersion,
    };
    const releasePath = path.join(output, "release.json");
    await writeFile(releasePath, `${JSON.stringify(release, null, 2)}\n`, "utf8");
    await utimes(releasePath, FIXED_TIMESTAMP, FIXED_TIMESTAMP);

    const audit = await auditPortableTree({
      probeRunner,
      repositoryRoot: repository,
      portableRoot: output,
      sourceRoots: [repository, vendor, path.dirname(application), path.dirname(sourceArchive)],
      peInspector,
    });
    return { rootPath: output, fileCount: audit.fileCount, toolCount: validated.toolCount };
  } catch (error) {
    await rm(output, { recursive: true, force: true });
    throw error;
  }
}

function parseCli(argv) {
  const [command, ...tokens] = argv;
  const options = { source: [] };
  for (let index = 0; index < tokens.length; index += 2) {
    const key = tokens[index];
    const value = tokens[index + 1];
    if (!key?.startsWith("--") || value === undefined) throw new Error(`Invalid command-line option: ${key ?? "<missing>"}`);
    const name = key.slice(2);
    if (name === "source") options.source.push(path.resolve(value));
    else options[name] = path.resolve(value);
  }
  return { command, options };
}

async function runCli() {
  const { command, options } = parseCli(process.argv.slice(2));
  const repositoryRoot = options.repo ?? process.cwd();
  if (command === "stage") {
    if (!options.app || !options.vendor || !options["gallery-dl-source"] || !options.output) {
      throw new Error("stage requires --app, --vendor, --gallery-dl-source, and --output");
    }
    return stagePortableTree({
      repositoryRoot,
      appPath: options.app,
      vendorRoot: options.vendor,
      galleryDlSourceArchive: options["gallery-dl-source"],
      tiktokCorePath:options["tiktok-core"],
      outputRoot: options.output,
    });
  }
  if (command === "audit") {
    if (!options.portable) throw new Error("audit requires --portable");
    return auditPortableTree({ repositoryRoot, portableRoot: options.portable, sourceRoots: options.source });
  }
  if (command === "hash") {
    if (!options.file) throw new Error("hash requires --file");
    await assertRegularFile(options.file, "hash input");
    return { sha256: await sha256File(options.file) };
  }
  throw new Error("Expected command: stage, audit, or hash");
}

const invokedDirectly = process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
if (invokedDirectly) {
  runCli()
    .then((result) => process.stdout.write(`${JSON.stringify(result)}\n`))
    .catch((error) => {
      process.stderr.write(`Portable packaging failed: ${error.message}\n`);
      process.exitCode = 1;
    });
}
