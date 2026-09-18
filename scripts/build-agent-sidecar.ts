import { mkdir, copyFile, chmod } from "node:fs/promises";
import { join, resolve } from "node:path";

const projectRoot = resolve(import.meta.dir, "..");
const tauriRoot = join(projectRoot, "src-tauri");
const binariesRoot = join(tauriRoot, "binaries");
const isWindows = process.platform === "win32";
const extension = isWindows ? ".exe" : "";
const profile = ["1", "true"].includes((process.env.TAURI_ENV_DEBUG ?? "").toLowerCase())
  ? "debug"
  : "release";

const run = (args: string[]) => {
  const result = Bun.spawnSync({ cmd: args, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) {
    throw new Error(`${args[0]} exited with code ${result.exitCode}`);
  }
};

const rustHost = () => {
  const result = Bun.spawnSync({ cmd: ["rustc", "-vV"], stdout: "pipe", stderr: "inherit" });
  if (result.exitCode !== 0) throw new Error("Unable to determine the Rust host target.");
  const output = new TextDecoder().decode(result.stdout);
  const host = output.match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) throw new Error("Rust did not report a host target.");
  return host;
};

const targetFromEnvironment = () => {
  const explicit = process.env.DEVSYNC_TARGET_TRIPLE ?? process.env.TAURI_ENV_TARGET_TRIPLE;
  if (explicit) return explicit;

  const platform = process.env.TAURI_ENV_PLATFORM;
  const arch = process.env.TAURI_ENV_ARCH;
  if (!platform || !arch) return rustHost();

  if (platform === "darwin") {
    if (arch === "universal") return "universal-apple-darwin";
    if (arch === "aarch64" || arch === "arm64") return "aarch64-apple-darwin";
    if (arch === "x86_64" || arch === "x64") return "x86_64-apple-darwin";
  }
  if (platform === "linux") {
    if (arch === "aarch64" || arch === "arm64") return "aarch64-unknown-linux-gnu";
    if (arch === "x86_64" || arch === "x64") return "x86_64-unknown-linux-gnu";
  }
  if (platform === "windows") {
    if (arch === "aarch64" || arch === "arm64") return "aarch64-pc-windows-msvc";
    if (arch === "x86_64" || arch === "x64") return "x86_64-pc-windows-msvc";
  }
  return rustHost();
};

const cargoBuild = (target: string) => {
  const args = [
    "cargo",
    "build",
    "--manifest-path",
    join(tauriRoot, "Cargo.toml"),
    "--bin",
    "devsync-agent",
    "--target",
    target,
  ];
  if (profile === "release") args.push("--release");
  const result = Bun.spawnSync({
    cmd: args,
    env: {
      ...process.env,
      // tauri-build validates externalBin during every Cargo build. The
      // sidecar itself is what this script is producing, so disable that
      // validation for this inner Agent-only build.
      TAURI_CONFIG: JSON.stringify({ bundle: { externalBin: [] } }),
    },
    stdout: "inherit",
    stderr: "inherit",
  });
  if (result.exitCode !== 0) {
    throw new Error(`${args[0]} exited with code ${result.exitCode}`);
  }
  return join(tauriRoot, "target", target, profile, `devsync-agent${extension}`);
};

const target = targetFromEnvironment();
await mkdir(binariesRoot, { recursive: true });

if (target === "universal-apple-darwin") {
  const armPath = cargoBuild("aarch64-apple-darwin");
  const intelPath = cargoBuild("x86_64-apple-darwin");
  const armStagedPath = join(binariesRoot, `devsync-agent-aarch64-apple-darwin${extension}`);
  const intelStagedPath = join(binariesRoot, `devsync-agent-x86_64-apple-darwin${extension}`);
  const stagedPath = join(binariesRoot, `devsync-agent-${target}${extension}`);

  await copyFile(armPath, armStagedPath);
  await copyFile(intelPath, intelStagedPath);
  run(["lipo", "-create", armPath, intelPath, "-output", stagedPath]);
  const universalTargetDirectory = join(tauriRoot, "target", target, profile);
  const universalTargetPath = join(universalTargetDirectory, `devsync-agent${extension}`);
  await mkdir(universalTargetDirectory, { recursive: true });
  await copyFile(stagedPath, universalTargetPath);
  if (!isWindows) {
    await chmod(armStagedPath, 0o755);
    await chmod(intelStagedPath, 0o755);
    await chmod(stagedPath, 0o755);
    await chmod(universalTargetPath, 0o755);
  }
  console.log(`Staged ${stagedPath}`);
} else {
  const builtPath = cargoBuild(target);
  const stagedPath = join(binariesRoot, `devsync-agent-${target}${extension}`);
  await copyFile(builtPath, stagedPath);
  if (!isWindows) await chmod(stagedPath, 0o755);
  console.log(`Staged ${stagedPath}`);
}
