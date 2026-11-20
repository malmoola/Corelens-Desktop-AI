/**
 * Opening a shell on a device: from the canvas menu, and
 * from the inspector's action row. One place, so both ask the same
 * questions in the same order and report the same refusals.
 */
import { ipc, type ConsoleLine } from '../lib/ipc';
import { describeLine, logAddress } from '../lib/consoleLine';
import { deviceInfoPatch, drawingsOf } from '../lib/deviceInfo';
import { planSsh, sshAddress } from '../lib/sshLaunch';
import { t } from '../i18n';
import { useStore } from '../state/store';
import type { DeviceNodeData } from '../types/domain';

/**
 * Opens a shell on a device and puts its tab in front.
 *
 * The two ways this fails before anything is sent — no address, no credential
 * of its own — are decided by `planSsh` and reported as themselves. A timeout
 * would be the wrong answer to both.
 *
 * With `via`, the session goes through that neighbour: this machine logs in
 * to it, and it to the device, with the same login.
 */
export async function openSsh(nodeId: string, data: DeviceNodeData, via?: { address: string; label: string }) {
  const store = useStore.getState();
  // The project's login is what a device with none of its own uses.
  const plan = planSsh(data, store.doc.credentialDefaults?.ssh);
  if (!plan.ok) {
    store.setStatusMessage(t(plan.reason === 'noAddress' ? 'ssh.noAddress' : 'ssh.noCredential'));
    return;
  }
  store.setStatusMessage(via ? t('ssh.openingThrough', { name: plan.label, via: via.label }) : t('ssh.opening', { name: plan.label }));
  store.requestPanelTab('ssh');
  // The panel's own event listener turns each login stage into the
  // status line while this open is pending, so the wait is legible instead
  // of a frozen "Connecting…".
  try {
    // A size the device can use straight away; the terminal tells it the real
    // one as soon as it has been laid out.
    const { keepaliveSeconds, logByDefault } = store.settings.terminal;
    const id = await ipc.sshOpen(plan.address, plan.credentialId, { cols: 120, rows: 30 }, undefined, keepaliveSeconds, via?.address);
    useStore.getState().openSshTab({ id, address: plan.address, label: plan.label, nodeId, status: 'open', credentialId: plan.credentialId, ...(via ? { via } : {}) });
    useStore.getState().setStatusMessage(null);
    // Logging every session without being asked each time.
    const folder = useStore.getState().settings.backupFolder;
    if (logByDefault && folder) {
      await ipc
        .sshLogStart(id, { folder, device: plan.label, address: plan.address, site: data.site ?? '' })
        .catch((e: unknown) => useStore.getState().setStatusMessage(e instanceof Error ? e.message : String(e)));
    }
  } catch (e: unknown) {
    useStore.getState().setStatusMessage(e instanceof Error ? e.message : String(e));
  }
}

/**
 * Opens a console on a port of this machine and puts its tab in front.
 * Resolves to why it could not, or null once the tab is open.
 *
 * Named after the device when it was asked for from one, after the port
 * otherwise. Logged the same as an SSH session, under the port's name.
 */
export async function openConsole(port: string, line: ConsoleLine, target?: { label?: string; nodeId?: string }): Promise<string | null> {
  const store = useStore.getState();
  store.setStatusMessage(t('console.opening', { port, line: describeLine(line) }));
  try {
    const id = await ipc.consoleOpen(port, line);
    const label = target?.label?.trim() || port;
    useStore.getState().openSshTab({ id, address: port, label, nodeId: target?.nodeId, status: 'open', console: line });
    useStore.getState().setStatusMessage(null);
    const { logByDefault } = useStore.getState().settings.terminal;
    const folder = useStore.getState().settings.backupFolder;
    if (logByDefault && folder) {
      await ipc
        .sshLogStart(id, { folder, device: label, address: logAddress(port), site: '' })
        .catch((e: unknown) => useStore.getState().setStatusMessage(e instanceof Error ? e.message : String(e)));
    }
    return null;
  } catch (e: unknown) {
    const why = e instanceof Error ? e.message : String(e);
    useStore.getState().setStatusMessage(why);
    return why;
  }
}

/**
 * Hands the device to whatever terminal the machine already has.
 *
 * The username goes with it and the password does not: every external client
 * that takes one takes it on a command line, where the rest of the machine can
 * read it. The client asks. Corelens then knows nothing about the session —
 * no transcript, no colouring, no keepalive, which is what the panel is for.
 */
export async function openSshElsewhere(data: DeviceNodeData) {
  const store = useStore.getState();
  const address = sshAddress(data);
  if (!address) {
    store.setStatusMessage(t('ssh.noAddress'));
    return;
  }
  try {
    const argv = await ipc.sshExternal(address, await sshUsername(data), undefined, store.settings.terminal.externalCommand);
    store.setStatusMessage(t('ssh.externalRan', { command: argv[0] ?? '' }));
  } catch (e: unknown) {
    store.setStatusMessage(e instanceof Error ? e.message : String(e));
  }
}

/**
 * The username an external client should be handed.
 *
 * It comes out of the vault's *summary* — the label and username of a saved
 * credential, which is not a secret and is already listed in Settings. The
 * password stays where it is. With no credential chosen there is nothing to
 * pass, and the client asks for both.
 */
async function sshUsername(data: DeviceNodeData): Promise<string> {
  const id = data.sshCredentialId?.trim();
  if (!id) return '';
  const saved = await ipc.listCredentials().catch(() => []);
  return saved.find((c) => c.id === id)?.username ?? '';
}

/**
 * Asks one device what it is and writes what it said into its fields.
 *
 * Not a crawl: one login to this device's own address with its own login
 * (or the project's), identity questions only, and no other device
 * contacted. The status line says what was asked, so whoever pressed it can
 * tell a network's owner exactly what was sent.
 */
export async function getDeviceInfo(nodeId: string, data: DeviceNodeData) {
  const store = useStore.getState();
  const plan = planSsh(data, store.doc.credentialDefaults?.ssh);
  if (!plan.ok) {
    store.setStatusMessage(t(plan.reason === 'noAddress' ? 'ssh.noAddress' : 'ssh.noCredential'));
    return;
  }
  store.setStatusMessage(t('deviceInfo.asking', { name: plan.label }));
  try {
    const info = await ipc.getDeviceInfo(plan.address, plan.credentialId);
    // The device as it is now, on every page it is drawn on: it may have been
    // edited, or the page switched, while this ran.
    const state = useStore.getState();
    const drawings = drawingsOf(state.doc.pages, nodeId, plan.address);
    if (drawings.length === 0) {
      state.setStatusMessage(t('deviceInfo.gone', { name: plan.label }));
      return;
    }
    const now = Date.now();
    const changedAll = new Set<string>();
    const patches = drawings.map(({ pageId, node }) => {
      const { patch, changed } = deviceInfoPatch(info, node.data as DeviceNodeData, now, plan.address);
      changed.forEach((c) => changedAll.add(c));
      return { pageId, nodeId: node.id, patch };
    });
    state.patchNodesOnAnyPage(patches, 'Get device info');
    const changed = [...changedAll];
    const asked = info.commands.join(', ');
    state.setStatusMessage(
      changed.length
        ? t('deviceInfo.filled', { name: plan.label, fields: changed.join(', '), asked })
        : t('deviceInfo.nothingNew', { name: plan.label, asked }),
    );
  } catch (e: unknown) {
    useStore.getState().setStatusMessage(t('deviceInfo.failed', { name: plan.label, why: e instanceof Error ? e.message : String(e) }));
  }
}
