import { getOctokit, context } from "@actions/github";

async function getAssetSign(url) {
  const response = await fetch(url, {
    method: "GET",
    headers: {
      "Content-Type": "application/octet-stream",
    },
  });

  // Without this check an HTTP error body ("Not Found", an S3 XML error, a
  // rate-limit page) is returned as if it were the signature/digest, gets
  // written into release.json, and every client then fails minisign
  // verification ("Invalid encoding in minisign data") — or, for Android,
  // compares its APK against a digest that was never a digest. Fail loudly
  // here instead: the caller's Promise.allSettled leaves the field unset, the
  // emit() guard drops the platform, and the updater simply reports no update.
  if (!response.ok) {
    throw new Error(
      `Failed to download ${url}: HTTP ${response.status} ${response.statusText}`
    );
  }

  return response.text();
}

async function createTauriRelease() {
  if (process.env.GITHUB_TOKEN === undefined) {
    throw new Error("GITHUB_TOKEN is not found!");
  }

  const github = getOctokit(process.env.GITHUB_TOKEN);
  const { repos } = github.rest;
  const repoMetaData = {
    owner: context.repo.owner,
    repo: context.repo.repo,
  };

  const tagsResult = await repos.listTags({ ...repoMetaData, per_page: 10, page: 1 });
  const latestTag = tagsResult.data.find((tag) => tag.name.startsWith("v"));
  console.log(latestTag);

  const latestRelease = await repos.getReleaseByTag({ ...repoMetaData, tag: latestTag.name });
  const latestAssets = latestRelease.data.assets;

  // Signed updater artifacts: Windows (.nsis.zip), Linux (.AppImage.tar.gz),
  // macOS universal (.app.tar.gz). darwin-x86_64 and darwin-aarch64 share
  // the same universal artifact. Android uses its own native UpdateChecker
  // (DownloadManager + APK install prompt) with a sha256 instead of minisign.
  const windowsX86_64 = {};
  const linuxX86_64 = {};
  const darwinUniversal = {};
  const android = {};

  const promises = latestAssets.map(async (asset) => {
    const { name, browser_download_url } = asset;

    if (/\.nsis\.zip$/.test(name)) {
      windowsX86_64.url = browser_download_url;
    }
    if (/\.nsis\.zip\.sig$/.test(name)) {
      windowsX86_64.signature = await getAssetSign(browser_download_url);
    }

    if (/\.AppImage\.tar\.gz$/.test(name)) {
      linuxX86_64.url = browser_download_url;
    }
    if (/\.AppImage\.tar\.gz\.sig$/.test(name)) {
      linuxX86_64.signature = await getAssetSign(browser_download_url);
    }

    if (/\.app\.tar\.gz$/.test(name)) {
      darwinUniversal.url = browser_download_url;
    }
    if (/\.app\.tar\.gz\.sig$/.test(name)) {
      darwinUniversal.signature = await getAssetSign(browser_download_url);
    }

    // Android: universal APK
    if (/prinny-android-universal\.apk$/.test(name)) {
      android.url = browser_download_url;
      android.version = latestTag.name;
    }
    if (/prinny-android-universal\.apk\.sha256$/.test(name)) {
      const sha256Text = await getAssetSign(browser_download_url);
      const digest = sha256Text.split(/\s+/)[0];
      // `sha256sum` output is "<64 hex>  <filename>". Anything else (an error
      // page that still returned 200, a truncated download) is not a digest
      // and must not reach the client that verifies against it.
      if (!/^[0-9a-f]{64}$/i.test(digest)) {
        throw new Error(`Malformed sha256 for ${name}: ${JSON.stringify(digest.slice(0, 80))}`);
      }
      android.sha256 = digest.toLowerCase();
    }
  });

  const settled = await Promise.allSettled(promises);
  for (const result of settled) {
    if (result.status === "rejected") {
      console.error(`Asset fetch failed: ${result.reason?.message ?? result.reason}`);
    }
  }

  const releaseData = {
    version: latestTag.name,
    notes: `https://github.com/${repoMetaData.owner}/${repoMetaData.repo}/releases/tag/${latestTag.name}`,
    pub_date: new Date().toISOString(),
    platforms: {},
  };

  // Each desktop platform is only emitted when BOTH the updater archive
  // and its .sig are present. Emitting with an empty signature crashes
  // the updater with "Invalid encoding in minisign data" on download.
  // The Tauri updater plugin deserializes every entry under `platforms`
  // as { signature, url } — adding android here would fail with
  // "missing field signature". Android lives at top-level instead and
  // is read by our native UpdateChecker.kt.
  const emit = (key, obj) => {
    if (obj.url && obj.signature) {
      releaseData.platforms[key] = obj;
    } else {
      console.log(`No signed ${key} updater artifact (TAURI_SIGNING_PRIVATE_KEY not set, or build failed?)`);
    }
  };
  emit('windows-x86_64', windowsX86_64);
  emit('linux-x86_64', linuxX86_64);
  emit('darwin-x86_64', darwinUniversal);
  emit('darwin-aarch64', darwinUniversal);

  // Android mirrors the desktop emit() guard: BOTH the APK url and its sha256
  // must be present. The client (UpdateChecker.kt) verifies the digest of the
  // downloaded APK, so an entry with a url and no sha256 either fails the
  // update or — worse, if a client ever treated a missing digest as "nothing
  // to check" — installs an unverified APK. No digest, no update offer.
  if (android.url && android.sha256) {
    releaseData.android = android;
  } else if (android.url) {
    console.log('Android APK found but no .sha256 digest — skipping android entry');
  } else {
    console.log('No android artifact');
  }

  // Get or create the "tauri" release used as updater metadata storage
  let tauriRelease;
  try {
    const result = await repos.getReleaseByTag({ ...repoMetaData, tag: 'tauri' });
    tauriRelease = result.data;
  } catch (err) {
    if (err.status === 404) {
      console.log('Creating tauri release for updater metadata...');
      tauriRelease = await repos.createRelease({
        ...repoMetaData,
        tag_name: 'tauri',
        name: 'Updater Metadata',
        body: 'Auto-generated release for Tauri updater metadata. Do not delete.',
        draft: false,
        prerelease: false,
      });
      tauriRelease = tauriRelease.data;
    } else {
      throw err;
    }
  }

  const prevReleaseAsset = tauriRelease.assets.find((asset) => asset.name === 'release.json');
  if (prevReleaseAsset) {
    await repos.deleteReleaseAsset({ ...repoMetaData, asset_id: prevReleaseAsset.id });
  }

  console.log(releaseData);
  await repos.uploadReleaseAsset({
    ...repoMetaData,
    release_id: tauriRelease.id,
    name: 'release.json',
    data: JSON.stringify(releaseData, null, 2),
  });
}
createTauriRelease();
