// electron-builder's configuration; electron/dist.mjs stages what it names and calls it, one architecture at a
// time (separate Apple silicon and Intel downloads, no universal one). Nothing signs without an identity given.
const path = require("node:path");

const stage = process.env.BRIGADIER_STAGE ?? "../../target/electron-stage/host";
const identity = process.env.APPLE_SIGNING_IDENTITY;
const feed = process.env.BRIGADIER_UPDATE_FEED;
const inStage = (name) => path.join(stage, name);

module.exports = {
  appId: "ai.brigadier.app",
  productName: "Brigadier",
  afterPack: "electron/afterPack.cjs",
  electronVersion: "45.2.3",
  npmRebuild: false,
  nodeGypRebuild: false,
  directories: { output: process.env.BRIGADIER_DIST ?? "../../target/electron-dist/host", buildResources: "electron/resources" },
  extraMetadata: {
    name: "brigadier",
    description: "Brigadier",
    homepage: "https://github.com/syncra-studio/brigadier-ai",
    main: "dist-electron/main.mjs",
    type: "module",
    ...(process.env.BRIGADIER_VERSION ? { version: process.env.BRIGADIER_VERSION } : {}),
  },
  asar: true,
  // Says which Mac it's for: separate Apple silicon and Intel downloads.
  artifactName: "${productName}-${version}-${arch}.${ext}",
  // Only the bundles and the page: nothing from node_modules, which main has bundled.
  // The dev control's chunk ships only in a check package, whose main can reach it.
  files: [
    "package.json",
    "dist-electron/*.mjs",
    ...(process.env.BRIGADIER_CHECK_PACKAGE ? [] : ["!dist-electron/devControl.mjs"]),
    "dist-electron/preload.cjs",
    { from: "dist-electron/page", to: "page" },
    "!**/node_modules/**",
  ],
  extraResources: [{ from: inStage("shell_native.node"), to: "shell_native.node" }],
  electronFuses: {
    runAsNode: false,
    enableCookieEncryption: true,
    enableNodeOptionsEnvironmentVariable: false,
    enableNodeCliInspectArguments: false,
    enableEmbeddedAsarIntegrityValidation: true,
    onlyLoadAppFromAsar: true,
    grantFileProtocolExtraPrivileges: false,
  },
  ...(feed ? { publish: [{ provider: "generic", url: feed }] } : { publish: null }),
  mac: {
    category: "public.app-category.developer-tools",
    icon: "src-tauri/icons/icon.icns",
    minimumSystemVersion: "14.0",
    // Sidecars stay in Contents/MacOS: computer use and recorded Claude hooks name that path.
    extraFiles: [
      { from: inStage("sidecars/brigadierd"), to: "MacOS/brigadierd" },
      // The CLI goes in afterPack.cjs, once the executable has moved off its name.
      { from: inStage("Brigadier Computer Use.app"), to: "Helpers/Brigadier Computer Use.app" },
    ],
    // Signed before the app: a cross-compiled binary comes with no signature at all, and the app's fails on it.
    binaries: ["Contents/MacOS/brigadierd", "Contents/MacOS/brigadier", "Contents/Resources/shell_native.node"],
    // An ad-hoc signature can't load an addon under the hardened runtime's library validation.
    identity: identity ?? "-",
    hardenedRuntime: Boolean(identity),
    gatekeeperAssess: false,
    notarize: Boolean(process.env.APPLE_ID && identity),
    entitlements: "electron/Entitlements.plist",
    entitlementsInherit: "electron/Entitlements.plist",
    target: ["dmg", "zip"],
    extendInfo: {
      NSMicrophoneUsageDescription: "Brigadier listens only while you dictate into the message box; your speech is turned into text on this Mac and stays on it.",
      NSCameraUsageDescription: "Only web pages you allow in Brigadier's browser can use the camera, like in any browser.",
      NSLocationUsageDescription: "Only web pages you allow in Brigadier's browser can know your location, like in any browser.",
      NSLocationWhenInUseUsageDescription: "Only web pages you allow in Brigadier's browser can know your location, like in any browser.",
      CFBundleDocumentTypes: [{ CFBundleTypeName: "Folder", CFBundleTypeRole: "Viewer", LSHandlerRank: "Alternate", LSItemContentTypes: ["public.folder"] }],
    },
  },
  dmg: { sign: false },
  win: {
    // "Brigadier" and the CLI "brigadier" would be one file on a case-insensitive disk.
    executableName: "brigadier-app",
    icon: "src-tauri/icons/icon.ico",
    target: ["nsis"],
    extraFiles: [
      { from: inStage("sidecars/brigadierd.exe"), to: "brigadierd.exe" },
      { from: inStage("sidecars/brigadier.exe"), to: "brigadier.exe" },
      { from: inStage("sidecars/brigadier-cu.exe"), to: "brigadier-cu.exe" },
    ],
  },
  // Per user, in %LOCALAPPDATA%\Programs\Brigadier: the uninstaller removes its folder recursively, and the data
  // is in %LOCALAPPDATA%\Brigadier\Data, which this must never contain (paths.ts, installFolderLeavesData).
  nsis: { oneClick: true, perMachine: false, allowToChangeInstallationDirectory: false, deleteAppDataOnUninstall: false, shortcutName: "Brigadier" },
  linux: {
    executableName: "brigadier-app",
    icon: "src-tauri/icons",
    target: ["deb", "AppImage"],
    category: "Development",
    maintainer: "Brigadier <noreply@brigadier.invalid>",
    extraFiles: [
      { from: inStage("sidecars/brigadierd"), to: "brigadierd" },
      { from: inStage("sidecars/brigadier"), to: "brigadier" },
      { from: inStage("sidecars/brigadier-cu"), to: "brigadier-cu" },
    ],
  },
};
