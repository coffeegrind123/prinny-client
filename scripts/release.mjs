import { getOctokit, context } from "@actions/github";

async function getAssetSign(url) {
  const response = await fetch(url, {
    method: "GET",
    headers: {
      "Content-Type": "application/octet-stream",
    },
  });

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
      android.sha256 = sha256Text.split(/\s+/)[0];
    }
  });

  await Promise.allSettled(promises);

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

  if (android.url) {
    releaseData.android = android;
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
