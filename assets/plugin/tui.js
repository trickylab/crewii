import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { initialTabs, newEvents, placementIndex } from './tabs.js';
import { roleOf, backlogOrder } from './core.js';

const directory = path.dirname(fileURLToPath(import.meta.url));
const configPath = path.join(directory, 'config.json');
const tabID = tab => tab.sessionID ?? tab.id;
const record = value => value !== null && typeof value === 'object' && !Array.isArray(value);

export function createTui({ fs, statePath, configPath, setInterval, clearInterval }) {
  function read() {
    try {
      const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
      if (!record(config) || typeof config.division !== 'string' || !/^[a-z0-9][a-z0-9_-]*$/i.test(config.division)) return { invalidConfig: true };
      const state = JSON.parse(fs.readFileSync(typeof statePath === 'function' ? statePath(config) : statePath, 'utf8'));
      if (!Array.isArray(config.roles) || !config.roles.every(role => record(role) && typeof role.id === 'string') ||
          !record(state) || !Number.isSafeInteger(state.seq) || state.seq < 0 || !record(state.instances) || !record(state.tickets) || !Array.isArray(state.events) ||
          !Object.values(state.instances).every(instance => record(instance) && (instance.sessionID === null || typeof instance.sessionID === 'string')) ||
          !Object.values(state.tickets).every(ticket => record(ticket) && typeof ticket.status === 'string' && typeof ticket.holder === 'string') ||
          !state.events.every(event => record(event) && Number.isSafeInteger(event.seq) && typeof event.type === 'string' &&
            (event.type !== 'assigned' || (typeof event.instance === 'string' && typeof event.ticket === 'string' && typeof event.from === 'string')))) return;
      return { config, state };
    } catch { return; }
  }

  return {
    id: 'crewii.tui',
    setup(ctx) {
      const tabs = ctx.ui.tabs;
      if (!tabs.enabled()) {
        ctx.ui.toast.show({ message: 'crewii: session tabs are off' });
        return;
      }
      const initial = read();
      if (initial?.invalidConfig) {
        ctx.ui.toast.show({ message: 'crewii: invalid config division' });
        return;
      }
      let lastSeq, orderAttempts = 0;
      function poll(data = read()) {
        if (!data || data.invalidConfig) return;
        const { config, state } = data;
        let moved = false;
        if (orderAttempts < 30 && config.roles.every(role => state.instances[`${role.id}-01`]?.sessionID)) {
          try {
            const current = tabs.list();
            const leads = config.roles.map(role => state.instances[`${role.id}-01`].sessionID);
            const index = leads.findIndex((id, i) => !current[i] || tabID(current[i]) !== id);
            if (index < 0) {
              orderAttempts = 30;
            } else {
              orderAttempts++;
              if (!current.some(tab => tabID(tab) === leads[index])) tabs.open(leads[index]);
              moved = true;
              tabs.move(leads[index], index);
            }
          } catch { /* Retry on the next poll. */ }
        }
        if (lastSeq === undefined) {
          for (const id of initialTabs(config, state)) {
            try { tabs.open(id); } catch { /* Ignore UI errors for this tab. */ }
          }
          lastSeq = state.seq;
          return;
        }
        lastSeq = Math.min(lastSeq, state.seq);
        let processedSeq = state.seq;
        try {
          for (const event of newEvents(state.events, lastSeq)) {
            try {
              if (event.type !== 'assigned') continue;
              const id = state.instances[event.instance]?.sessionID;
              if (!id) continue;
              const alreadyOpen = tabs.list().some(tab => tabID(tab) === id);
              if (!alreadyOpen && moved) {
                processedSeq = event.seq - 1;
                break;
              }
              tabs.open(id);
              if (!alreadyOpen) {
                const names = Object.fromEntries(Object.entries(state.instances).map(([name, instance]) => [instance.sessionID, name]));
                const list = tabs.list().map(tab => ({ sessionID: tabID(tab), title: names[tabID(tab)] }));
                moved = true;
                tabs.move(id, placementIndex(list, event.instance));
              }
              ctx.ui.toast.show({ message: `${event.instance}: ${event.ticket} from ${event.from}` });
            } catch { /* Continue with the next event. */ }
          }
        } finally { lastSeq = processedSeq; }
      }
      ctx.keymap.layer(() => ({ commands: [{
        id: 'crewii.crew', title: 'Crew', palette: true, slash: { name: 'crew' },
        async run() {
          const data = read();
          if (!data || data.invalidConfig) {
            ctx.ui.toast.show({ message: 'crewii: no crew state yet' });
            return;
          }
          const { state } = data;
          const options = Object.entries(state.instances).filter(([, instance]) => instance.sessionID).map(([name, instance]) => {
            const tickets = Object.entries(state.tickets).filter(([, ticket]) => ticket.status === 'open' && ticket.holder === name).map(([id]) => id);
            return { title: `${name} · ${tickets.join(', ') || 'idle'}`, value: instance.sessionID };
          });
          options.push(...Object.entries(state.tickets).filter(([, ticket]) => ticket.status === 'backlog').sort((a, b) => backlogOrder(a[1], b[1])).map(([id, ticket]) => ({ title: `${id} · ${ticket.role || roleOf(ticket.holder)}${ticket.urgent ? ' urgent' : ''} · waiting ${Math.floor((Date.now() - ticket.created) / 60_000)}m`, value: '' })));
          const selected = await ctx.ui.dialog.select({ title: 'Crew', options });
          if (selected) tabs.focus(selected);
        },
      }] }));
      poll(initial);
      const timer = setInterval(() => poll(), 1000);
      return () => clearInterval(timer);
    },
  };
}

export default createTui({ fs, configPath, statePath: config => path.join(directory, '../../../../state', `${config.division}.json`), setInterval, clearInterval });
