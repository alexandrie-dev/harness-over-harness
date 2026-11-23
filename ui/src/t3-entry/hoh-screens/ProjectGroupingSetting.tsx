import { useSyncExternalStore } from 'react';

import { Switch } from '~/components/ui/switch';
import {
  getGroupAgentsByProject,
  setGroupAgentsByProject,
  subscribeGroupAgentsByProject,
} from './grouping';

/** HoH's home-only appearance preference, injected into T3's Appearance page by vite.ts. */
export function ProjectGroupingSetting() {
  const grouped = useSyncExternalStore(
    subscribeGroupAgentsByProject,
    getGroupAgentsByProject,
    () => false,
  );
  return (
    <section className="rounded-xl border border-border/70 bg-card/40 px-3 py-3 sm:px-4" aria-labelledby="hoh-project-grouping-title">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <h3 id="hoh-project-grouping-title" className="text-sm font-medium text-foreground">Group agents by project</h3>
          <p className="mt-1 text-xs leading-5 text-muted-foreground">Show project folders on Home. Hover or open a folder to reach its agents.</p>
        </div>
        <Switch
          aria-label="Group agents by project"
          checked={grouped}
          onCheckedChange={(checked) => setGroupAgentsByProject(Boolean(checked))}
        />
      </div>
    </section>
  );
}
