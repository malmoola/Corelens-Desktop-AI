/**
 * The checks as the engine is given them.
 *
 * The engine runs what it is sent and knows nothing of the diagram. A device
 * or a link put in maintenance on the diagram was shown as Maintenance, and
 * its checks went on being run as if nothing had been said: the engine
 * logged the device going down and coming back, and the availability report
 * counted a planned reload as an outage. So what is sent says it: a check
 * is in maintenance when it is ticked so itself, or the thing it checks is.
 */
import type { ProjectDocument } from '../state/store';
import type { Probe } from '../types/domain';
import { allEdges, allNodes } from './pages';

/** The ids of the devices and links in maintenance. */
function inMaintenance(doc: Pick<ProjectDocument, 'pages'>): Set<string> {
  const ids = new Set<string>();
  for (const n of allNodes(doc)) if ((n.data as { maintenance?: boolean } | undefined)?.maintenance) ids.add(n.id);
  for (const e of allEdges(doc)) if ((e.data as { maintenance?: boolean } | undefined)?.maintenance) ids.add(e.id);
  return ids;
}

/** Every enabled check the engine can run, for this project, each saying
 *  whether it or what it checks is in maintenance. */
export function probesForEngine(doc: Pick<ProjectDocument, 'pages' | 'probes'>, projectId: string): Probe[] {
  const quiet = inMaintenance(doc);
  return doc.probes
    .filter((p) => p.enabled && p.kind !== 'manual')
    .map((p) => ({ ...p, projectId, maintenance: Boolean(p.maintenance) || quiet.has(p.objectId) }));
}

/**
 * What of the diagram, beside the checks themselves, changes what the
 * engine should be running: which checked things are in maintenance.
 * The same string until that changes.
 */
export function maintenanceKey(doc: Pick<ProjectDocument, 'pages' | 'probes'>): string {
  const quiet = inMaintenance(doc);
  if (quiet.size === 0) return '';
  return [...new Set(doc.probes.map((p) => p.objectId))].filter((id) => quiet.has(id)).sort().join('\u0000');
}
