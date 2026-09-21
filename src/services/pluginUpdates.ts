export type PluginUpdateEntry = {
  id: string; name: string; version: string; installed_version: string;
  source_id: string; source_name: string; source_url: string;
  installed_source_id?: string; install_source_status?: string;
  installed: boolean; registered: boolean; enabled: boolean; effective_enabled: boolean;
  update_available: boolean; install_type?: string; auth_required?: boolean; auth_configured?: boolean;
  revision?: number; installed_revision?: number;
  upgrade_allowed?: boolean; upgrade_block_reason?: string;
  platforms?: { goos: string; goarch: string }[];
};
export type PluginStoreSnapshot = {
  plugins_enabled: boolean;
  plugins: PluginUpdateEntry[];
  source_errors?: { source_name: string; source_url: string; message: string }[];
};
export type PluginPlatform = { assetOs: string; assetArch: string };

export function installedPluginEntries(snapshot: PluginStoreSnapshot): PluginUpdateEntry[] {
  // Alternate sources can share an ID. Never turn a normal update into a source switch.
  return snapshot.plugins.filter((entry) => entry.installed
    && entry.install_source_status === 'matched'
    && Boolean(entry.source_id) && entry.installed_source_id === entry.source_id);
}

export function pluginUpdateBlock(entry: PluginUpdateEntry, snapshot: PluginStoreSnapshot, platform: PluginPlatform | null) {
  if (!installedPluginEntries(snapshot).includes(entry)) return 'source' as const;
  if (!snapshot.plugins_enabled || !entry.enabled) return 'disabled' as const;
  if (!platform) return 'platform' as const;
  const arch = platform.assetArch === 'aarch64' ? 'arm64' : platform.assetArch;
  if (entry.platforms?.length && !entry.platforms.some((value) => value.goos === platform.assetOs && value.goarch === arch)) return 'platform' as const;
  if (entry.auth_required && !entry.auth_configured) return 'auth' as const;
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(entry.id)
    || !/^[0-9][0-9A-Za-z.+-]*$/.test(entry.version)) return 'version' as const;
  // The backend compares selected artifact identity and installed revisions.
  // Missing support must not turn a stale registry into an offered downgrade.
  if (entry.upgrade_allowed !== true) return 'revision' as const;
  return null;
}

export function pluginInstallRequest(entry: PluginUpdateEntry, snapshot: PluginStoreSnapshot, platform: PluginPlatform | null) {
  if (pluginUpdateBlock(entry, snapshot, platform)) throw new Error('Plugin is not eligible for an update from its installed source');
  return {
    path: `/plugin-store/${encodeURIComponent(entry.id)}/install`,
    body: { version: entry.version, upgrade_only: true },
    options: { query: { source: entry.source_id }, timeoutMs: 300000 },
  };
}

export function pluginHasUpdate(entry: PluginUpdateEntry) {
  return entry.upgrade_allowed === true
    && entry.version.replace(/^v/, '') !== entry.installed_version.replace(/^v/, '')
    && (entry.install_type === 'direct' || entry.update_available);
}

export function pluginVersionLoaded(payload: { plugins?: { id: string; registered: boolean; effective_enabled: boolean; metadata?: { version?: string } }[] }, id: string, version: string) {
  return payload.plugins?.some((plugin) => plugin.id === id && plugin.registered
    && plugin.effective_enabled && plugin.metadata?.version?.replace(/^v/, '') === version.replace(/^v/, '')) ?? false;
}
