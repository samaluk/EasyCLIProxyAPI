import { describe, expect, it } from 'bun:test';
import { installedPluginEntries, pluginInstallRequest, pluginUpdateBlock, pluginVersionLoaded, pluginHasUpdate,
  type PluginStoreSnapshot, type PluginUpdateEntry } from '../src/services/pluginUpdates';

const entry: PluginUpdateEntry = {
  id: 'reviewed-plugin', name: 'Reviewed plugin', installed: true, registered: true,
  enabled: true, effective_enabled: true, installed_version: '1.0.0', version: '1.0.1',
  source_id: 'source_123', source_name: 'Reviewed', source_url: 'https://example.org/registry.json',
  installed_source_id: 'source_123', install_source_status: 'matched', update_available: true,
  upgrade_allowed: true, revision: 101, installed_revision: 100,
  platforms: [{ goos: 'darwin', goarch: 'arm64' }],
};
const snapshot: PluginStoreSnapshot = { plugins_enabled: true, plugins: [entry] };
const platform = { assetOs: 'darwin', assetArch: 'aarch64' };

describe('installed plugin updates', () => {
  it('does not advertise alternate sources, assumed identities or new installs', () => {
    expect(installedPluginEntries({ ...snapshot, plugins: [entry,
      { ...entry, source_id: 'official', install_source_status: 'different' },
      { ...entry, install_source_status: 'assumed', installed_source_id: '' },
      { ...entry, installed: false },
    ] })).toEqual([entry]);
  });
  it('pins the exact version and passes source separately from the URL path', () => {
    expect(pluginInstallRequest(entry, snapshot, platform)).toEqual({
      path: '/plugin-store/reviewed-plugin/install', body: { version: '1.0.1', upgrade_only: true },
      options: { query: { source: 'source_123' }, timeoutMs: 300000 },
    });
  });
  it('blocks disabled plugins, missing source auth and unsupported platforms', () => {
    expect(pluginUpdateBlock(entry, { ...snapshot, plugins_enabled: false }, platform)).toBe('disabled');
    for (const modified of [{ ...entry, enabled: false }, { ...entry, auth_required: true, auth_configured: false }]) {
      expect(() => pluginInstallRequest(modified, { ...snapshot, plugins: [modified] }, platform)).toThrow();
    }
    expect(pluginUpdateBlock(entry, snapshot, { assetOs: 'linux', assetArch: 'amd64' })).toBe('platform');
  });
  it('rejects path or version injection before invoking management', () => {
    for (const modified of [{ ...entry, id: '../other' }, { ...entry, version: 'latest' }]) {
      expect(() => pluginInstallRequest(modified, { ...snapshot, plugins: [modified] }, platform)).toThrow();
    }
  });
  it('requires actual registered runtime metadata before claiming the new version is active', () => {
    expect(pluginVersionLoaded({ plugins: [{ id: entry.id, registered: true, effective_enabled: true, metadata: { version: '1.0.0' } }] }, entry.id, '1.0.1')).toBe(false);
    expect(pluginVersionLoaded({ plugins: [{ id: entry.id, registered: true, effective_enabled: true, metadata: { version: 'v1.0.1' } }] }, entry.id, '1.0.1')).toBe(true);
  });
});


it('offers a reviewed revision only when the backend confirms its artifact is safe', () => {
  expect(pluginHasUpdate({ ...entry, install_type: 'direct', installed_version: '1.0.0-review.ffabcd0.20260920', version: '1.0.0-review.00abcd0.20260921', update_available: false })).toBe(true);
  expect(pluginHasUpdate({ ...entry, install_type: 'direct', installed_version: '1.0.1', version: '1.0.1' })).toBe(false);
  for (const upgrade_allowed of [false, undefined]) {
    const stale = { ...entry, install_type: 'direct', version: '1.0.0-review.older', installed_version: '1.0.0-review.newer', upgrade_allowed };
    expect(pluginHasUpdate(stale)).toBe(false);
    expect(pluginUpdateBlock(stale, { ...snapshot, plugins: [stale] }, platform)).toBe('revision');
    expect(() => pluginInstallRequest(stale, { ...snapshot, plugins: [stale] }, platform)).toThrow();
  }
  const same = { ...entry, version: entry.installed_version };
  expect(pluginInstallRequest(same, { ...snapshot, plugins: [same] }, platform).body.upgrade_only).toBe(true);
});
