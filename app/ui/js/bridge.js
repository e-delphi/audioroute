// Ponte com o Rust (app/host/src/bridge.rs) pelas mensagens do WebView2.
//
//   Pedido:   { id, cmd, args }           JS → Rust
//   Resposta: { id, ok, value | error }   Rust → JS
//   Evento:   { event, payload }          Rust → JS
//
// As telas usam só o objeto Api. Na etapa de rede, outra implementação
// (HTTP/WebSocket) poderá controlar um mestre remoto sem mudar as telas.

const Bridge = (() => {
  const webview = window.chrome && window.chrome.webview
  const pending = new Map()
  const listeners = new Map()
  let nextId = 1

  if (webview) {
    webview.addEventListener('message', (e) => {
      const msg = typeof e.data === 'string' ? JSON.parse(e.data) : e.data
      if (!msg) return
      if ('id' in msg) {
        const p = pending.get(msg.id)
        if (!p) return
        pending.delete(msg.id)
        if (msg.ok) p.resolve(msg.value)
        else p.reject(msg.error || 'erro desconhecido')
      } else if (msg.event) {
        for (const cb of listeners.get(msg.event) || []) cb(msg.payload)
      }
    })
  }

  return {
    /** Chama um comando do Rust e espera a resposta. */
    call(cmd, args = {}) {
      if (!webview) return Promise.reject('fora do app: abra pelo audioroute.exe')
      const id = nextId++
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject })
        webview.postMessage(JSON.stringify({ id, cmd, args }))
      })
    },
    /** Assina um evento enviado pelo Rust; retorna o cancelamento. */
    on(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set())
      listeners.get(event).add(cb)
      return () => listeners.get(event).delete(cb)
    },
  }
})()

const Api = {
  role: () => Bridge.call('role'),
  setRole: (role) => Bridge.call('set_role', { role }),
  /** Desliga o áudio e volta para a tela inicial. */
  resetRole: () => Bridge.call('reset_role'),
  devices: () => Bridge.call('devices'),
  routes: () => Bridge.call('routes'),
  refresh: () => Bridge.call('refresh'),
  connect: (from, to) => Bridge.call('connect', { from, to }),
  disconnect: (from, to) => Bridge.call('disconnect', { from, to }),
  setGain: (from, to, gain) => Bridge.call('set_gain', { from, to, gain }),
  setMute: (from, to, mute) => Bridge.call('set_mute', { from, to, mute }),
  setDelay: (id, delayMs) => Bridge.call('set_delay', { id, delayMs }),
  setDescription: (id, text) => Bridge.call('set_description', { id, text }),
  setMode: (id, mode) => Bridge.call('set_mode', { id, mode }),
  /** null = período padrão do modo. */
  setPeriod: (id, periodMs) => Bridge.call('set_period', { id, periodMs }),
  /** Torna a saída o padrão do Windows (onde os outros programas tocam). */
  setWindowsOutput: (id) => Bridge.call('set_windows_output', { id }),
  loadLayout: async () => JSON.parse(await Bridge.call('load_layout')),
  saveLayout: (layout) => Bridge.call('save_layout', { layout: JSON.stringify(layout) }),
  /** Picos (0–1) por dispositivo, ~20 vezes por segundo. */
  onLevels: (cb) => Bridge.on('levels', cb),
  /** Estado dos cabos, 1 vez por segundo. */
  onStats: (cb) => Bridge.on('stats', (s) => cb(s || [])),
  /** Um cabo ligou ou caiu sozinho, ou um dispositivo foi plugado/desplugado. */
  onChanged: (cb) => Bridge.on('changed', () => cb()),
  /**
   * Estado da rede: { role: 'controller', peers: [{ name, address, devices }], address, port }
   * ou { role: 'participant', status: { state, manual, controller, address, streams, error } }.
   */
  network: () => Bridge.call('network'),
  /**
   * Pares virtuais do driver: { max, pairs: [{ pair, name, speaker, mic }] },
   * ou null sem o driver. speaker/mic são os ids dos nós no canvas.
   */
  virtualDevices: () => Bridge.call('virtual_devices'),
  /** Usa um par livre. Devolve { warnings } (o que o Windows não aceitou). */
  virtualAdd: (name) => Bridge.call('virtual_add', { name }),
  virtualRename: (pair, name) => Bridge.call('virtual_rename', { pair, name }),
  virtualRemove: (pair) => Bridge.call('virtual_remove', { pair }),
  /** Firewall do Windows: { ok, network, problem }. */
  firewall: () => Bridge.call('firewall'),
  /** Libera o audioroute no firewall (o Windows pede administrador). */
  allowFirewall: () => Bridge.call('allow_firewall'),
  /** Participante: endereço do controlador digitado (null: busca automática). */
  setControllerAddress: (address) => Bridge.call('set_controller_address', { address }),
  /** O mesmo, 1 vez por segundo. */
  onNetwork: (cb) => Bridge.on('network', cb),
}
