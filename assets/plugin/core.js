export function emptyState() {
  return { version: 1, seq: 0, nextTicket: 1, instances: {}, tickets: {}, events: [], locks: {} };
}

export function instanceName(role, n) {
  return `${role}-${String(n).padStart(2, '0')}`;
}

export function roleOf(name) {
  return name.replace(/-\d+$/, '');
}

export const backlogOrder = (a, b) => Number(!!b.urgent) - Number(!!a.urgent) || (a.backlogAt ?? a.created) - (b.backlogAt ?? b.created);

export function parseTarget(to, config) {
  if (to.includes('/')) return { kind: 'error', message: 'Cross-division requests are not supported; choose a local role.' };
  if (to.startsWith('ses_')) return { kind: 'session', sessionID: to };
  const role = roleOf(to);
  if (!config.roles.some(item => item.id === role)) return { kind: 'error', message: `Unknown role: ${role}; choose a role from the roster.` };
  return role === to ? { kind: 'role', role } : { kind: 'instance', name: to };
}

export function route(state, config, role) {
  const names = Object.keys(state.instances).filter(name => state.instances[name].role === role).sort((a, b) => Number(a.slice(role.length + 1)) - Number(b.slice(role.length + 1)));
  const open = Object.values(state.tickets).filter(ticket => ticket.status === 'open' && names.includes(ticket.holder));
  const capacity = role === 'ba' ? config.limits.ticketsPerBa : 1;
  const free = names.find(name => open.filter(ticket => ticket.holder === name).length < capacity);
  if (free) return { instance: free };
  if (names.length < config.limits.instancesPerRole) {
    let n = 1;
    while (names.includes(instanceName(role, n))) n++;
    return { create: instanceName(role, n) };
  }
  return { backlog: true };
}

export function openTicket(state, { requester, requesterName, holder, text, now }) {
  const id = `T-${state.nextTicket}`;
  const ticket = { id, requester, requesterName, holder, status: 'open', title: [...text.replace(/\s+/g, ' ').trim()].slice(0, 60).join(''), created: now, closed: null };
  return { state: { ...state, nextTicket: state.nextTicket + 1, tickets: { ...state.tickets, [id]: ticket } }, ticket };
}

export function closeTicket(state, id, now) {
  if (!state.tickets[id]) throw new Error(`Unknown ticket ${id}; choose an existing ticket.`);
  if (state.tickets[id].status === 'closed') return state;
  return { ...state, tickets: { ...state.tickets, [id]: { ...state.tickets[id], status: 'closed', closed: now } } };
}

export function addEvent(state, event, now) {
  const seq = state.seq + 1;
  return { ...state, seq, events: [...state.events, { ...event, seq, created: now }].slice(-100) };
}

export function senderTag({ name, sessionID, ticket }) {
  return `[from ${name} ${sessionID} ticket ${ticket}]`;
}

export function counterpart(state, ticketId, senderName) {
  const ticket = state.tickets[ticketId];
  if (!ticket) throw new Error(`Unknown ticket ${ticketId}; choose an existing ticket.`);
  return senderName === ticket.holder ? ticket.requester : ticket.holder;
}

export function rosterText(config, state) {
  const roles = config.roles.map(role => `${role.id}: ${role.description}`).join('\n');
  const instances = Object.keys(state.instances).join(', ');
  return `Use crew_send: to accepts a role id (routes to a free instance), instance name (e.g. reviewer-02), or session id. Reply by passing ticket from the [from …] line; use final: true when delivering the result.\nUse crew_status to check progress; never open a ticket just to ask for status.\n${roles}${instances ? `\nInstances: ${instances}` : ''}`;
}
