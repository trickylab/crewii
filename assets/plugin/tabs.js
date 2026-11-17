import { roleOf, instanceName } from './core.js';

export function placementIndex(tabs, name) {
  const others = tabs.filter(tab => tab.title !== name);
  const index = others.findLastIndex(tab => roleOf(tab.title ?? '') === roleOf(name));
  return index < 0 ? others.length : index + 1;
}

export function newEvents(events, lastSeq) {
  return events.filter(event => event.seq > lastSeq).sort((a, b) => a.seq - b.seq);
}

export function initialTabs(config, state) {
  const names = [
    ...config.roles.map(role => instanceName(role.id, 1)),
    ...Object.values(state.tickets).filter(ticket => ticket.status === 'open').map(ticket => ticket.holder),
  ];
  return [...new Set(names.map(name => state.instances[name]?.sessionID).filter(Boolean))];
}
