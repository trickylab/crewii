import * as nodeFs from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseTarget, openTicket, closeTicket, addEvent, senderTag, counterpart, rosterText, roleOf, backlogOrder } from './core.js';
import { createStore } from './store.js';

const pluginDir = dirname(fileURLToPath(import.meta.url));
const errorResult = error => ({ content: `error: ${error.message ?? error}` });
const notFound = error => /NotFound/.test(`${error?.name ?? ''} ${error?._tag ?? ''}`) || error?.status === 404;
const message = (tag, text) => `${tag}\n<<<\n${text.replace(/^\[from /gm, '> [from ')}\n>>>`;
// ponytail: cap delegation at four tickets; raise only with an explicit routing policy.
const MAX_DEPTH = 4;
// ponytail: final delivery waits at most 60 seconds; make configurable if delivery requirements change.
const FINAL_TIMEOUT_MS = 60_000;
const RESET_SUMMARY = 'crewii context reset: your previous tickets are closed or reassigned; do not resume them. Start each new ticket only from its BRIEF. Use crew_status for crew state.';

export function createPlugin({ fs = nodeFs, now = Date.now, configPath = resolve(pluginDir, 'config.json'), statePath, tmpPath = resolve(pluginDir, '../../../../tmp'), reportPath = resolve(pluginDir, '../../../../reports'), setInterval = globalThis.setInterval, clearInterval = globalThis.clearInterval } = {}) {
  return { id: 'crewii', async setup(ctx) {
    try {
      const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
      if (config?.limits && config.limits.staleMinutes === undefined) config.limits.staleMinutes = 15;
      if (!config || typeof config.division !== 'string' || !/^[a-z0-9][a-z0-9_-]*$/i.test(config.division) || typeof config.agentPrefix !== 'string' || !Array.isArray(config.roles) || !config.roles.length || config.roles.some(role => !role || typeof role.id !== 'string' || !role.id || typeof role.description !== 'string') || !config.roles.some(role => role.id === 'ba') || !config.limits || !Number.isInteger(config.limits.instancesPerRole) || config.limits.instancesPerRole < 1 || !Number.isInteger(config.limits.ticketsPerBa) || config.limits.ticketsPerBa < 1) throw Error('Invalid crewii config');
      if (!Number.isInteger(config.limits.staleMinutes) || config.limits.staleMinutes < 0) throw Error('Invalid staleMinutes; use a non-negative integer');
      // ponytail: single server per location assumed; store serializes in-process only. Upgrade: file lock in store.js if multiple servers share a division.
      let reset = false, recovering;
      const store = createStore(statePath ?? resolve(pluginDir, '../../../../state', `${config.division}.json`), { fs, now, onReset: () => { reset = true; } });
      function cleanTicket(id) {
        if (!/^T-[0-9]+$/.test(id)) return;
        const dir = resolve(tmpPath, `${config.division}-${id}`);
        try { fs.rmSync(dir, { recursive: !fs.lstatSync(dir).isSymbolicLink(), force: true }); } catch (error) { if (error.code !== 'ENOENT') console.error(`crewii: ${error.message ?? error}`); }
      }
      function saveReport(ticket, text) {
        const file = resolve(reportPath, `${config.division}-${ticket.id}.md`);
        try {
          fs.mkdirSync(reportPath, { recursive: true });
          fs.writeFileSync(file, `# ${ticket.id} ${ticket.title}\nholder: ${ticket.holder}\nrequester: ${ticket.requesterName}\nparent: ${ticket.parent ?? '-'}\nclosed: ${new Date(now()).toISOString()}\n\n${text}\n`);
          return `\n\n(saved: ${file})`;
        } catch (error) { console.error(`crewii: ${error.message ?? error}`); return ''; }
      }
      const creating = new Map();
      const finalizing = new Set();
      const delivering = new Set();
      const directory = ctx.location.directory;
      let ready = false;

      const sessionOwners = new Map();
      const resetting = new Map();
      // ponytail: resets also clear direct (ticketless) tab chat; add a keep-context limit if users need it.
      async function resetContext(names, last) {
        for (const name of new Set(names)) {
          const state = store.read(), sessionID = state.instances[name]?.sessionID;
          if (typeof ctx.session.compact !== 'function' || typeof ctx.session.hook !== 'function' || !sessionID || Object.values(state.tickets).some(ticket => ticket.status === 'open' && ticket.holder === name)) continue;
          resetting.set(sessionID, RESET_SUMMARY + (last ? `\n\nLast ticket: ${last.id} ${last.title}\nFinal result:\n${last.text.slice(0, 2000)}` : ''));
          try { await ctx.session.compact({ sessionID, delivery: 'queue' }); } catch (error) { resetting.delete(sessionID); console.error(`crewii: ${error.message ?? error}`); }
        }
      }
      if (typeof ctx.session.hook === 'function') ctx.session.hook('compaction', event => {
        const summary = resetting.get(event.sessionID); resetting.delete(event.sessionID);
        if (!summary) return;
        const state = store.read(), owner = Object.keys(state.instances).find(name => state.instances[name].sessionID === event.sessionID);
        if (Object.values(state.tickets).some(ticket => ticket.status === 'open' && ticket.holder === owner && new RegExp(`ticket ${ticket.id}(?![0-9])`).test(JSON.stringify(event.messages)))) return;
        event.result = { summary };
      });
      const lastActivity = new Map(), lastPersist = new Map(), awaiting = new Set(), notified = new Set();
      async function ownerOf(sessionID) {
        const instances = store.read().instances;
        let id = sessionID, owner;
        for (let hops = 0; id && hops <= 4; hops++) {
          owner = Object.keys(instances).find(name => instances[name].sessionID === id) ?? sessionOwners.get(id);
          if (owner && instances[owner]) break;
          try { id = (await ctx.session.get({ sessionID: id })).parentID; } catch { return; }
        }
        if (!owner || !instances[owner]) return;
        sessionOwners.set(sessionID, owner);
        return owner;
      }
      // ponytail: shell writes and generators bypass this edit-tool lock; extend only with native shell enforcement.
      if (typeof ctx.permission?.hook === 'function') ctx.permission.hook('evaluate', async event => {
        const owner = await ownerOf(event.sessionID);
        if (!owner) return;
        lastActivity.set(owner, now());
        if (now() - (lastPersist.get(owner) ?? -Infinity) >= 60_000) {
          lastPersist.set(owner, now());
          await store.update(state => { if (state.instances[owner]) state.instances[owner].active = now(); return state; });
        }
        if (event.action !== 'edit') return;
        for (const resource of event.resources) {
          let root = resolve(directory, resource);
          while (!fs.existsSync(resolve(root, '.git'))) {
            const parent = dirname(root);
            if (parent === root) { root = null; break; }
            root = parent;
          }
          if (!root) continue;
          await store.update(state => {
            const holder = (state.locks ?? {})[root];
            const tickets = Object.values(state.tickets).filter(ticket => ticket.status === 'open' && ticket.holder === holder);
            if (holder && holder !== owner && tickets.length) {
              event.effect = 'deny'; event.message = `${root} is being written by ${holder} (${tickets.map(ticket => ticket.id).join(', ')}); wait, or send this work to ${holder}`;
              return state;
            }
            return { ...state, locks: { ...state.locks, [root]: owner } };
          });
          if (event.effect === 'deny') return;
        }
      });

      async function roster() {
        const state = store.read(), value = rosterText(config, state);
        for (const instance of Object.values(state.instances)) if (instance.sessionID) {
          try { await ctx.session.instructions.entry.put({ sessionID: instance.sessionID, key: 'crewii-roster', value }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
        }
      }
      async function ensureSession(name, preserveTicket) {
        if (creating.has(name)) return creating.get(name);
        let freed = false;
        const promise = (async () => {
          const instance = store.read().instances[name];
          if (instance.sessionID) {
            try { await ctx.session.get({ sessionID: instance.sessionID }); return instance.sessionID; } catch (error) {
              if (!notFound(error)) throw error;
              let dropped = [];
              const existing = new Set(Object.values(store.read().tickets).filter(ticket => ticket.status === 'open' && ticket.holder === name && ticket.id !== preserveTicket && !delivering.has(ticket.id)).map(ticket => ticket.id));
              await store.update(state => {
                dropped = Object.values(state.tickets).filter(ticket => ticket.status === 'open' && ticket.holder === name && existing.has(ticket.id) && !delivering.has(ticket.id));
                for (const ticket of dropped) state = closeTicket(state, ticket.id, now());
                return state;
              });
              for (const ticket of dropped) {
                cleanTicket(ticket.id);
                const state = store.read(), requester = state.instances[ticket.requesterName] ?? Object.values(state.instances).find(item => item.sessionID === ticket.requester);
                if (requester?.sessionID && ticket.requester !== 'user') try { await ctx.session.prompt({ sessionID: requester.sessionID, text: `${ticket.id} was dropped: ${name} session was lost; resend it if still needed.`, delivery: 'queue' }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
              }
              freed = dropped.length > 0;
            }
          }
          const created = await ctx.session.create({ title: name, agent: `${config.agentPrefix}${instance.role}`, location: { directory } });
          const sessionID = created.id ?? created.data?.id;
          if (!sessionID) throw Error(`Could not create session ${name}`);
          await store.update(state => ({ ...state, instances: { ...state.instances, [name]: { ...state.instances[name], sessionID } } }));
          if (ready) await roster();
          return sessionID;
        })();
        creating.set(name, promise);
        try { return await promise; } finally { creating.delete(name); if (freed && ready) queueMicrotask(() => { dispatch().catch(error => console.error(`crewii: ${error.message ?? error}`)); }); }
      }

      function choose(state, role, blocked, preferred, excluded = [], sticky = false) {
        const names = Object.keys(state.instances).filter(name => state.instances[name].role === role);
        const available = name => !blocked.has(name) && Object.values(state.tickets).filter(ticket => ticket.status === 'open' && ticket.holder === name).length < (role === 'ba' ? config.limits.ticketsPerBa : 1);
        if (sticky) return names.includes(preferred) && !excluded.includes(preferred) && available(preferred) ? preferred : undefined;
        const free = names.filter(available).sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
        const allowed = free.filter(name => !excluded.includes(name));
        if (allowed.length) return allowed.includes(preferred) ? preferred : allowed[0];
        if (names.length < config.limits.instancesPerRole) {
          let n = 1; while (state.instances[`${role}-${String(n).padStart(2, '0')}`] || excluded.includes(`${role}-${String(n).padStart(2, '0')}`) || blocked.has(`${role}-${String(n).padStart(2, '0')}`)) n++;
          const name = `${role}-${String(n).padStart(2, '0')}`;
          state.instances[name] = { role, sessionID: null, created: now() };
          return name;
        }
        return undefined;
      }
      async function deliver(ticket) {
        const targetID = await ensureSession(ticket.holder, ticket.id);
        const tag = senderTag({ name: ticket.requesterName, sessionID: ticket.requester, ticket: ticket.id });
        if (store.read().tickets[ticket.id]?.status !== 'open') return;
        await ctx.session.prompt({ sessionID: targetID, text: message(ticket.parent ? `${tag.slice(0, -1)} for ${ticket.parent}]` : tag, `${ticket.text ?? ticket.title}${ticket.excluded?.length ? '\n\nThis ticket was reassigned after inactivity.' : ''}${ticket.handover ? `\n\nPrevious holder's last update:\n${ticket.handover}` : ''}`), delivery: 'queue' });
        await store.update(state => {
          if (state.tickets[ticket.id]?.status !== 'open' || state.tickets[ticket.id].holder !== ticket.holder) return state;
          lastActivity.set(ticket.holder, now());
          return addEvent(state, { type: 'assigned', instance: ticket.holder, ticket: ticket.id, from: ticket.requesterName }, now());
        });
      }
      let dispatching, dispatchAgain = false;
      async function dispatch() {
        if (dispatching) { dispatchAgain = true; return dispatching; }
        dispatching = (async () => {
          do {
          dispatchAgain = false;
          const ids = Object.values(store.read().tickets).filter(ticket => ticket.status === 'backlog').sort(backlogOrder).map(ticket => ticket.id);
          for (const id of ids) {
            let assigned;
            await store.update(state => {
              const ticket = state.tickets[id]; if (!ticket || ticket.status !== 'backlog') return state;
              const blocked = new Set([ticket.requesterName]);
              for (let parent = ticket.parent; parent; parent = state.tickets[parent]?.parent) { const ancestor = state.tickets[parent]; if (!ancestor || ancestor.status !== 'open') return state; blocked.add(ancestor.holder); }
              if (ticket.sticky && !state.instances[ticket.preferred]) { delete ticket.sticky; delete ticket.preferred; }
              const holder = choose(state, ticket.role || roleOf(ticket.holder), blocked, ticket.preferred, ticket.excluded, ticket.sticky);
              if (holder) { assigned = { ...ticket, holder, status: 'open' }; state.tickets[id] = assigned; delivering.add(id); }
              return state;
            });
            if (assigned) try { await deliver(assigned); } catch (error) { console.error(`crewii: ${error.message ?? error}`); await store.update(state => { if (state.tickets[id]?.status === 'open') state.tickets[id] = { ...state.tickets[id], status: 'backlog', holder: '' }; return state; }); } finally { delivering.delete(id); }
          }
          } while (dispatchAgain);
        })().finally(() => { dispatching = undefined; });
        return dispatching;
      }
      async function assign({ to, text, sender, sessionID, parent, urgent }) {
        let assignment;
        try {
        const snapshot = store.read(), address = parseTarget(to, config);
        const named = address.kind === 'instance' ? address.name : address.kind === 'session' ? Object.keys(snapshot.instances).find(name => snapshot.instances[name].sessionID === address.sessionID) : undefined;
        const targetRole = address.kind === 'role' ? address.role : snapshot.instances[named]?.role;
        if (targetRole === 'ba' && sender !== 'user') throw Error('Only the user opens ba tickets; reply on your ticket instead.');
        if (targetRole && !parent && sender !== 'user' && roleOf(sender) !== 'ba') throw Error('Only ba opens new tickets; reply on your ticket, or pass ticket to open a linked sub-ticket.');
        await dispatch();
        if (named && snapshot.instances[named]?.sessionID && Object.values(snapshot.tickets).some(ticket => ticket.status === 'open' && ticket.holder === named)) await ensureSession(named);
        await store.update(state => {
          const target = parseTarget(to, config);
          if (target.kind === 'error') throw Error(target.message);
          if (parent && finalizing.has(parent)) throw Error(`${parent} is already closing`);
          const ancestors = [];
          for (let id = parent; id; id = state.tickets[id]?.parent) {
            if (ancestors.some(ticket => ticket.id === id) || ancestors.length >= MAX_DEPTH) throw Error('delegation too deep; finish or reply instead');
            const ticket = state.tickets[id];
            if (!ticket || ticket.status !== 'open') throw Error('ticket must exist and be open');
            ancestors.push(ticket);
          }
          if (ancestors.length >= MAX_DEPTH) throw Error('delegation too deep; finish or reply instead');
          const preferred = target.kind === 'role' ? Object.entries(state.locks ?? {}).filter(([root, owner]) => text.includes(root) && state.instances[owner] && roleOf(owner) === target.role).sort((a, b) => b[0].length - a[0].length)[0]?.[1] : target.kind === 'instance' ? target.name : Object.keys(state.instances).find(name => state.instances[name].sessionID === target.sessionID);
          if (target.kind !== 'role' && !state.instances[preferred]) throw Error(`Unknown instance; valid roles: ${config.roles.map(role => role.id).join(', ')}`);
          const role = target.kind === 'role' ? target.role : state.instances[preferred].role;
          const sticky = target.kind === 'role' && preferred && Object.values(state.tickets).some(ticket => ticket.status === 'open' && ticket.holder === preferred);
          const holder = choose(state, role, new Set([...ancestors.map(ticket => ticket.holder), sender]), preferred, [], sticky);
          const opened = openTicket(state, { requester: sender === 'user' ? 'user' : sessionID, requesterName: sender, holder: holder ?? '', text, now: now() });
          Object.assign(opened.ticket, { role, text, status: holder ? 'open' : 'backlog', ...(parent ? { parent } : {}), ...(preferred ? { preferred } : {}), ...(sticky ? { sticky: true } : {}), ...(urgent ? { urgent: true } : {}) });
          assignment = { holder, ticket: opened.ticket.id };
          if (holder) delivering.add(assignment.ticket);
          return opened.state;
        });
        try {
          if (!assignment.holder) return assignment;
          await deliver(store.read().tickets[assignment.ticket]);
        } catch (error) {
          await store.update(state => {
            state = closeTicket(state, assignment.ticket, now());
            if (state.instances[assignment.holder] && !state.instances[assignment.holder].sessionID && !Object.values(state.tickets).some(ticket => ticket.status === 'open' && ticket.holder === assignment.holder)) {
              const instances = { ...state.instances }; delete instances[assignment.holder];
              state = { ...state, instances };
            }
            return state;
          });
          cleanTicket(assignment.ticket);
          throw error;
        }
        return assignment;
        } finally { if (assignment) delivering.delete(assignment.ticket); }
      }

      async function recover() {
        if (recovering) return recovering;
        store.read();
        if (!reset) return;
        if (!recovering) recovering = (async () => { reset = false; await ensureLeads(); await roster(); })().finally(() => { recovering = undefined; });
        await recovering;
      }

      function checkFinal(state, id) {
        const current = state.tickets[id];
        if (!current || current.status !== 'open') throw Error('already closed');
        const children = Object.values(state.tickets).filter(ticket => ticket.parent === id && ['open', 'backlog'].includes(ticket.status)).map(ticket => ticket.id);
        if (children.length) throw Error(`${id} still has open sub-tickets: ${children.join(', ')}`);
        return state;
      }

      const tool = { name: 'crew_send', description: 'Send a request or reply to a crew member.', input: { type: 'object', properties: { to: { type: 'string', minLength: 1 }, text: { type: 'string', minLength: 1 }, ticket: { type: 'string', pattern: '^T-[0-9]+$' }, final: { type: 'boolean' }, urgent: { type: 'boolean' } }, required: ['to', 'text'], additionalProperties: false }, async execute(input, context) {
        try {
          await recover();
          if (typeof input.to !== 'string' || !input.to.trim()) throw Error('to must be a non-empty string');
          if (typeof input.text !== 'string' || !input.text.trim()) throw Error('text must be a non-empty string');
          if (input.ticket !== undefined && (typeof input.ticket !== 'string' || !/^T-[0-9]+$/.test(input.ticket))) throw Error('ticket must match T-<number>');
          if (input.final !== undefined && typeof input.final !== 'boolean') throw Error('final must be a boolean');
          if (input.urgent !== undefined && typeof input.urgent !== 'boolean') throw Error('urgent must be a boolean');
          if (input.final && !input.ticket) throw Error('final needs a ticket');
          if (Object.keys(input).some(key => !['to', 'text', 'ticket', 'final', 'urgent'].includes(key))) throw Error('unexpected input property');
          const state = store.read(), sender = Object.keys(state.instances).find(name => state.instances[name].sessionID === context.sessionID);
          if (!sender) throw Error('crew_send is only for crew sessions');
          if (input.to === sender || input.to === context.sessionID) throw Error('cannot send to yourself');
          const address = input.ticket && input.to === 'user' ? { kind: 'user' } : parseTarget(input.to, config);
          if (address.kind === 'error') throw Error(address.message);
          if (address.kind === 'instance' && !state.instances[address.name]) throw Error(`Unknown instance; valid roles: ${config.roles.map(role => role.id).join(', ')}`);
          if (input.ticket !== undefined) {
            const ticket = state.tickets[input.ticket];
            if (!ticket) throw Error('ticket must exist and be open');
            if (ticket.status === 'cancelled' || ticket.status === 'backlog') throw Error('ticket is not open; wait for assignment');
            if (sender !== ticket.holder && context.sessionID !== ticket.requester && sender !== ticket.requesterName) throw Error('sender must be ticket holder or requester');
            if (ticket.status === 'closed' && input.final) throw Error('already closed');
            const target = counterpart(state, input.ticket, sender);
            const targetName = state.instances[target] ? target : state.instances[ticket.requesterName] && target === ticket.requester ? ticket.requesterName : Object.keys(state.instances).find(name => state.instances[name].sessionID === target);
            const reply = (address.kind === 'user' && target === 'user') || (address.kind === 'session' && (address.sessionID === target || address.sessionID === state.instances[targetName]?.sessionID)) || (address.kind === 'instance' && address.name === targetName) || (address.kind === 'role' && targetName && address.role === roleOf(targetName));
            if (reply) {
              if (input.urgent !== undefined) throw Error('urgent applies to new tickets');
              const reopened = ticket.status === 'closed' && sender !== ticket.holder;
              if (reopened) await store.update(s => ({ ...s, tickets: { ...s.tickets, [input.ticket]: { ...s.tickets[input.ticket], status: 'open', closed: null } } }));
              if (target === 'user') {
                let saved = '';
                if (input.final) { await store.update(s => closeTicket(checkFinal(s, input.ticket), input.ticket, now())); saved = saveReport(ticket, input.text); cleanTicket(input.ticket); await resetContext([sender], { id: ticket.id, title: ticket.title, text: input.text }); await dispatch(); }
                else awaiting.add(input.ticket);
                return { content: `Delivered to the user on ${input.ticket} (shown in this tab).\n\n${input.text}${saved}` };
              }
              const targetID = targetName ? await ensureSession(targetName, input.ticket) : target;
              const text = message(senderTag({ name: sender, sessionID: context.sessionID, ticket: input.ticket }), input.text);
              if (input.final) {
                await store.update(state => { checkFinal(state, input.ticket); if (finalizing.has(input.ticket)) throw Error('already closed'); finalizing.add(input.ticket); return state; });
                let timer;
                try {
                  const saved = saveReport(ticket, input.text);
                  const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(Error('delivery timed out; retry')), FINAL_TIMEOUT_MS); timer.unref?.(); });
                  await Promise.race([ctx.session.prompt({ sessionID: targetID, text: text + saved, delivery: 'queue' }), timeout]);
                  await store.update(state => closeTicket(checkFinal(state, input.ticket), input.ticket, now()));
                  cleanTicket(input.ticket);
                } finally { clearTimeout(timer); finalizing.delete(input.ticket); }
                await resetContext([sender], { id: ticket.id, title: ticket.title, text: input.text });
                await dispatch();
              } else {
                await ctx.session.prompt({ sessionID: targetID, text, delivery: sender !== ticket.holder && targetName === ticket.holder ? 'steer' : 'queue' });
                if (sender === ticket.holder) awaiting.add(input.ticket); else awaiting.delete(input.ticket);
              }
              return { content: `Sent to ${targetName ?? target} on ${input.ticket}${reopened ? ' (reopened)' : ''}` };
            }
            if (input.final) throw Error('final applies to replies; omit it when handing work to another role');
          }
          const result = await assign({ to: input.to, text: input.text, sender, sessionID: context.sessionID, parent: input.ticket, urgent: input.urgent });
          return { content: result.holder ? `Sent to ${result.holder} as ${result.ticket}${input.ticket ? ` (for ${input.ticket})` : ''}` : `Backlogged ${result.ticket}; waiting for capacity` };
        } catch (error) { return errorResult(error); }
      } };
      const status = { name: 'crew_status', description: 'Check crew progress.', input: { type: 'object', properties: {}, additionalProperties: false }, async execute(input, context) {
        try {
          await recover();
          const state = store.read();
          if (!Object.values(state.instances).some(instance => instance.sessionID === context.sessionID)) throw Error('crew_status is only for crew sessions');
          const tickets = Object.values(state.tickets), age = time => Math.floor((now() - time) / 60_000);
          const open = tickets.filter(ticket => ticket.status === 'open');
          return { content: [...open.map(ticket => `${ticket.id} ${ticket.holder} age=${age(ticket.created)}m requester=${ticket.requesterName}${ticket.parent ? ` parent=${ticket.parent}` : ''} ${ticket.title}`), `instances: ${Object.keys(state.instances).map(name => { const ticket = open.find(ticket => ticket.holder === name); if (!ticket) return `${name}(idle)`; const minutes = age(state.instances[name].active ?? lastActivity.get(name) ?? ticket.created); return `${name}(busy, active ${minutes}m ago${config.limits.staleMinutes > 0 && minutes >= config.limits.staleMinutes ? ' stale' : ''})`; }).join(', ')}`, ...tickets.filter(ticket => ticket.status === 'backlog').sort(backlogOrder).map(ticket => `${ticket.id} ${ticket.role || roleOf(ticket.holder)}${ticket.urgent ? ' urgent' : ''} backlog waiting ${age(ticket.created)}m from ${ticket.requesterName} ${ticket.title}`), ...tickets.filter(ticket => ticket.status === 'cancelled').sort((a, b) => b.closed - a.closed).slice(0, 5).map(ticket => `${ticket.id} ${ticket.role || roleOf(ticket.holder)} cancelled ${ticket.reason ?? '-'}`), ...tickets.filter(ticket => ticket.status === 'closed').sort((a, b) => b.closed - a.closed).slice(0, 5).map(ticket => `${ticket.id} ${ticket.holder} closed ${age(ticket.closed)}m ago ${ticket.title}`)].join('\n') };
        } catch (error) { return errorResult(error); }
      } };
      const command = { name: 'crew-ask', description: 'Ask the crew.', async execute({ sessionID, prompt }) {
        try {
          await recover();
          if (typeof prompt?.text !== 'string' || !prompt.text.trim()) throw Error('text must be a non-empty string');
          const result = await assign({ to: 'ba', text: prompt.text, sender: 'user', sessionID });
          if (store.read().instances[result.holder]?.sessionID !== sessionID) await ctx.session.synthetic({ sessionID, text: `${result.ticket} -> ${result.holder ?? 'backlog'}` });
          return { content: `${result.ticket} -> ${result.holder ?? 'backlog'}` };
        } catch (error) {
          const result = errorResult(error);
          try { await ctx.session.synthetic({ sessionID, text: result.content }); } catch { /* caller may be unavailable */ }
          return result;
        }
      } };
      async function interrupt(sessionID) {
        if (sessionID && typeof ctx.session.interrupt === 'function') try { await ctx.session.interrupt({ sessionID, resume: false }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
      }
      const cancel = { name: 'crew_cancel', description: 'Cancel a requested ticket.', input: { type: 'object', properties: { ticket: { type: 'string', pattern: '^T-[0-9]+$' }, reason: { type: 'string', minLength: 1 } }, required: ['ticket', 'reason'], additionalProperties: false }, async execute(input, context) {
        try {
          await recover();
          if (typeof input.ticket !== 'string' || !/^T-\d+$/.test(input.ticket) || typeof input.reason !== 'string' || !input.reason.trim() || Object.keys(input).some(key => !['ticket', 'reason'].includes(key))) throw Error('invalid cancellation; provide ticket and reason');
          let cancelled = [];
          await store.update(state => {
            const ticket = state.tickets[input.ticket], sender = Object.keys(state.instances).find(name => state.instances[name].sessionID === context.sessionID);
            if (!ticket || !['open', 'backlog'].includes(ticket.status)) throw Error('ticket is not open or backlogged; choose an active ticket');
            if (!sender || (ticket.requesterName !== sender && ticket.requester !== context.sessionID && !(sender === ticket.holder && roleOf(sender) === 'ba'))) throw Error('only the requester may cancel; ask the requester');
            const visit = id => { const item = state.tickets[id]; if (!item || !['open', 'backlog'].includes(item.status)) return; cancelled.push({ ...item }); state.tickets[id] = { ...item, status: 'cancelled', closed: now(), reason: input.reason }; for (const child of Object.values(state.tickets)) if (child.parent === id) visit(child.id); };
            visit(input.ticket); return state;
          });
          for (const ticket of cancelled) {
            cleanTicket(ticket.id);
            awaiting.delete(ticket.id);
            const instance = store.read().instances[ticket.holder];
            if (ticket.status === 'open' && instance?.sessionID && instance.sessionID !== context.sessionID) {
              if (instance.role !== 'ba') await interrupt(instance.sessionID);
              try { await ctx.session.synthetic({ sessionID: instance.sessionID, text: `${ticket.id} cancelled: ${input.reason}; keep your worktree and changes.` }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
            }
          }
          await resetContext(cancelled.filter(ticket => ticket.status === 'open').map(ticket => ticket.holder));
          await dispatch();
          return { content: `Cancelled ${input.ticket}${cancelled.length > 1 ? ` (+ sub-tickets ${cancelled.slice(1).map(ticket => ticket.id).join(', ')})` : ''}` };
        } catch (error) { return errorResult(error); }
      } };
      async function checkStale() {
        async function idle(sessionID) {
          try { const result = await ctx.session.context({ sessionID }); const messages = Array.isArray(result) ? result : result?.data; return Array.isArray(messages) && messages.at(-1)?.type === 'idle'; } catch { return false; }
        }
        if (!config.limits.staleMinutes) return;
        for (const ticket of Object.values(store.read().tickets)) {
          const state = store.read(), current = state.tickets[ticket.id], instance = state.instances[current?.holder];
          if (current?.status !== 'open' || current.holder !== ticket.holder || !instance || instance.role === 'ba' || delivering.has(ticket.id) || finalizing.has(ticket.id) || awaiting.has(ticket.id) || Object.values(state.tickets).some(child => child.parent === ticket.id && child.status === 'open') || now() - (lastActivity.get(ticket.holder) ?? ticket.created) < config.limits.staleMinutes * 60_000) continue;
          if (!await idle(instance.sessionID)) continue;
          let assigned, unavailable = false, reserved = false;
          await store.update(state => {
            const item = state.tickets[ticket.id];
            if (item?.status !== 'open' || item.holder !== ticket.holder || delivering.has(item.id) || finalizing.has(item.id) || awaiting.has(item.id) || now() - (lastActivity.get(item.holder) ?? item.created) < config.limits.staleMinutes * 60_000 || Object.values(state.tickets).some(child => child.parent === item.id && child.status === 'open')) return state;
            const role = item.role || roleOf(item.holder), blocked = new Set([item.holder, item.requesterName]);
            for (let parent = item.parent; parent; parent = state.tickets[parent]?.parent) { const ancestor = state.tickets[parent]; if (!ancestor || ancestor.status !== 'open') return state; blocked.add(ancestor.holder); }
            const excluded = [...new Set([...(item.excluded ?? []), item.holder])];
            const existing = new Set(Object.keys(state.instances));
            const holder = choose(state, role, blocked, undefined, excluded);
            if (!holder) { unavailable = true; return state; }
            reserved = !existing.has(holder);
            assigned = { ...item, role, holder, excluded }; state.tickets[item.id] = assigned; delivering.add(item.id); return state;
          });
          const requester = state.instances[ticket.requesterName]?.sessionID ?? ticket.requester;
          if (unavailable && !notified.has(ticket.id)) {
            notified.add(ticket.id);
            if (requester && requester !== 'user') try { await ctx.session.prompt({ sessionID: requester, text: `${ticket.id}: ${ticket.holder} inactive ${Math.floor((now() - (lastActivity.get(ticket.holder) ?? ticket.created)) / 60_000)}m; no other ${ticket.role || roleOf(ticket.holder)} instance is free`, delivery: 'queue' }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
          }
          if (!assigned) continue;
          if (!await idle(instance.sessionID)) {
            await store.update(state => { if (state.tickets[ticket.id]?.status === 'open' && state.tickets[ticket.id].holder === assigned.holder) state.tickets[ticket.id] = current; if (reserved && state.instances[assigned.holder]?.sessionID === null && !Object.values(state.tickets).some(item => item.status === 'open' && item.holder === assigned.holder)) delete state.instances[assigned.holder]; return state; });
            delivering.delete(ticket.id); continue;
          }
          await interrupt(instance.sessionID);
          const latest = store.read().tickets[ticket.id];
          if (latest?.status !== 'open' || latest.holder !== assigned.holder) { delivering.delete(ticket.id); continue; }
          try {
            const result = await ctx.session.context({ sessionID: instance.sessionID }), entries = Array.isArray(result) ? result : result?.data;
            const entry = entries?.findLast(item => (item.role ?? item.info?.role) === 'assistant');
            const handover = entry?.parts?.filter(part => part.type === 'text').map(part => part.text).join('\n').trim().slice(0, 1500);
            if (handover) await store.update(state => { if (state.tickets[ticket.id]?.status === 'open' && state.tickets[ticket.id].holder === assigned.holder) { assigned.handover = handover; state.tickets[ticket.id].handover = handover; } return state; });
          } catch { /* context is optional */ }
          await resetContext([ticket.holder]);
          const currentAssignment = store.read().tickets[ticket.id];
          if (currentAssignment?.status !== 'open' || currentAssignment.holder !== assigned.holder) { delivering.delete(ticket.id); continue; }
          if (requester && requester !== 'user') try { await ctx.session.prompt({ sessionID: requester, text: `${ticket.id} was reassigned after ${ticket.holder} became inactive.`, delivery: 'queue' }); } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
          try { await deliver(assigned); } catch (error) { console.error(`crewii: ${error.message ?? error}`); await store.update(state => { if (state.tickets[ticket.id]?.status === 'open' && state.tickets[ticket.id].holder === assigned.holder) state.tickets[ticket.id] = { ...state.tickets[ticket.id], status: 'backlog', holder: '', backlogAt: -now() }; return state; }); } finally { delivering.delete(ticket.id); }
          await dispatch();
        }
      }
      await ctx.tool.transform(editor => { editor.add(status); editor.add(cancel); editor.add(tool); });
      await ctx.command.transform(editor => editor.add(command));
      async function ensureLeads() {
      let page = await ctx.session.list({ directory, limit: 200 });
      let sessions = [...(Array.isArray(page) ? page : page.data ?? page.items ?? [])];
      while (page.cursor?.next) {
        page = await ctx.session.list({ cursor: page.cursor.next, limit: 200 });
        sessions.push(...(page.data ?? page.items ?? []));
      }
      for (const role of config.roles) {
        try {
          const name = `${role.id}-01`, current = store.read().instances[name];
          let live = false;
           if (current?.sessionID) try { await ctx.session.get({ sessionID: current.sessionID }); live = true; } catch (error) { if (!notFound(error)) throw error; await ensureSession(name); live = true; }
          if (!live) {
            const adopted = sessions.find(session => !session.parentID && session.title === name && session.agent === `${config.agentPrefix}${role.id}`);
            await store.update(state => ({ ...state, instances: { ...state.instances, [name]: { role: role.id, sessionID: adopted?.id ?? null, created: current?.created ?? now() } } }));
            if (!adopted) await ensureSession(name);
          }
        } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
      }
      }
      const abandoned = [];
      await store.update(state => {
        for (const [name, instance] of Object.entries(state.instances)) if (!instance.sessionID) {
          for (const ticket of Object.values(state.tickets)) if (ticket.status === 'open' && ticket.holder === name) { state = closeTicket(state, ticket.id, now()); abandoned.push(ticket.id); }
          delete state.instances[name];
        }
        return state;
      });
      for (const id of abandoned) cleanTicket(id);
      await ensureLeads();
      reset = false;
      await roster();
      ready = true;
      await dispatch();
      let checking;
      const timer = setInterval(() => checking ??= checkStale().catch(error => console.error(`crewii: ${error.message ?? error}`)).finally(() => { checking = undefined; }), 60_000);
      timer.unref?.();
      return () => clearInterval(timer);
    } catch (error) { console.error(`crewii: ${error.message ?? error}`); }
  } };
}

export default createPlugin();
