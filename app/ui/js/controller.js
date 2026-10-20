// Tela do controlador principal: os dispositivos são nós e as rotas são cabos.
// Arrastar de uma entrada (conector à direita) para uma saída (à esquerda)
// cria o cabo; clicar num cabo ou nó abre o painel lateral.
// O nó fixo "Windows" tem sempre um cabo até a saída padrão do sistema:
// levar a ponta dele para outra saída troca o padrão do Windows.

const Controller = {
  mount(root, onReset) {
    const GRID = Graph.GRID
    const COLUMN = { input: 0, output: 24 * GRID }
    const ROW_HEIGHT = 4 * GRID
    /** Id do nó fixo "Windows" e do cabo dele (não são dispositivos). */
    const WINDOWS = 'windows'
    const WINDOWS_CABLE = 'windows->default'

    const state = {
      devices: [],
      routes: [],
      stats: {},
      layout: {},
      showLoopbacks: true,
      selection: null, // { kind: 'device' | 'cable', id }
      fitted: false,
      virtual: null, // { max, pairs: [{ pair, name, speaker, mic }] }: pares do driver
    }

    // ---- elementos ----

    const loopbacksButton = h('button', { class: 'active', onClick: toggleLoopbacks }, '🔁 Loopbacks')
    const addPairButton = h(
      'button',
      { hidden: true, title: 'Cria um alto-falante e um microfone virtuais com o nome que você escolher', onClick: addPair },
      '➕ Par virtual',
    )
    const network = h('span', { class: 'net' })
    const canvas = h('div')
    const side = h('div', { class: 'side', hidden: true })
    root.append(
      h(
        'div',
        { class: 'router' },
        h(
          'header',
          { class: 'toolbar' },
          h('strong', null, 'audioroute'),
          h('span', { class: 'role' }, 'Controlador principal'),
          network,
          h('span', { class: 'spacer' }),
          addPairButton,
          loopbacksButton,
          h('button', { onClick: () => act(() => Api.refresh()) }, '⟳ Atualizar dispositivos'),
          switchRoleButton(
            onReset,
            'Todo o áudio do audioroute será desligado. Os cabos ficam salvos e voltam quando você escolher o papel de novo.',
          ),
        ),
        firewallBanner('Os outros computadores não vão conseguir se conectar a este.'),
        h('div', { class: 'workspace' }, canvas, side),
      ),
    )

    const graph = Graph.create(canvas, {
      canConnect,
      onConnect,
      onNodeClick: (id) => select(id === WINDOWS ? null : { kind: 'device', id }),
      onCableClick: (id) => select(id === WINDOWS_CABLE ? null : { kind: 'cable', id }),
      onPaneClick: () => select(null),
      onNodeMoved,
    })

    // ---- dados derivados ----

    const byId = () => new Map(state.devices.map((d) => [d.id, d]))
    const pairOf = (d) => (d && state.virtual?.pairs.find((p) => p.speaker === d.id || p.mic === d.id)) || null
    const isVirtualSpeaker = (d) => !!d && !!state.virtual?.pairs.some((p) => p.speaker === d.id)
    const isVirtual = (d) => !!pairOf(d)
    // A saída do Windows: uma saída deste computador, ou o alto-falante
    // virtual (aí o som dos programas entra no canvas por ele).
    const windowsOutput = () =>
      state.devices.find((d) => d.isDefault && d.available && !d.node && (d.kind === 'output' || isVirtualSpeaker(d)))
    const nameOf = (id) => byId().get(id)?.name ?? id

    function visibleDevices() {
      const cabled = new Set(state.routes.flatMap((r) => [r.from, r.to]))
      // O botão "Loopbacks" pode escondê-los; os que têm cabo sempre aparecem.
      return state.devices.filter((d) => !d.loopback || state.showLoopbacks || cabled.has(d.id))
    }

    // ---- nós ----

    function deviceNode(d) {
      const variant = d.loopback ? 'loopback' : d.kind === 'output' ? 'output' : 'input'
      const icon = { input: '🎤', output: '🔊', loopback: '🔁' }[variant]
      const subtitle =
        d.description ||
        (isVirtualSpeaker(d) && 'Os programas tocam nele; entra aqui') ||
        (isVirtual(d) && 'Sai como microfone nos programas') ||
        { input: 'Entrada', output: 'Saída', loopback: 'Captura o que toca nesta saída' }[variant]
      const selected = state.selection?.kind === 'device' && state.selection.id === d.id
      return h(
        'div',
        { class: ['device-node', variant, !d.available && 'unavailable', selected && 'selected'], 'data-live': d.id },
        d.kind === 'output' && h('div', { class: 'handle in' }),
        h('div', { class: 'icon' }, icon),
        h(
          'div',
          { class: 'body' },
          h('div', { class: 'name', title: d.name }, d.name),
          h('div', { class: 'subtitle', title: subtitle }, subtitle),
          h('div', { class: 'meter' }, h('div', { class: 'fill', 'data-meter': d.id })),
        ),
        h(
          'div',
          { class: 'badges' },
          d.node && h('span', { class: 'badge node', title: `Dispositivo do computador ${d.node}, pela rede` }, `💻 ${d.node}`),
          isVirtual(d) &&
            h('span', { class: 'badge virtual', title: 'Dispositivo virtual do driver do audioroute' }, `virtual ${pairOf(d).pair + 1}`),
          d.isDefault && h('span', { class: 'badge', title: isVirtualSpeaker(d) ? 'O Windows toca nele' : '' }, 'padrão'),
          d.mode === 'exclusive' && h('span', { class: 'badge exclusive' }, 'exclusivo'),
          d.stream && h('span', { class: 'badge', title: d.stream.format }, `${d.stream.periodMs.toFixed(1)} ms`),
          d.delayMs > 0 && h('span', { class: 'badge delay' }, `+${d.delayMs} ms`),
          !d.available && h('span', { class: 'badge off' }, 'desconectado'),
        ),
        d.kind === 'input' && h('div', { class: 'handle out' }),
      )
    }

    /** Som de todos os programas; o medidor mostra o que toca na saída padrão. */
    function windowsNode(out) {
      return h(
        'div',
        { class: 'windows-node', 'data-live': out?.id ?? '' },
        h('div', { class: 'icon' }, '🪟'),
        h(
          'div',
          { class: 'body' },
          h('div', { class: 'name' }, 'Windows'),
          h('div', { class: 'subtitle', title: out?.name ?? '' }, out ? `Tocando em ${out.name}` : 'Sem saída padrão'),
          h('div', { class: 'meter' }, h('div', { class: 'fill', 'data-meter': out?.id ?? '' })),
        ),
        h('div', { class: 'handle out' }),
      )
    }

    function renderNodes() {
      const rows = { left: 0, right: 0 }
      const list = visibleDevices().map((d) => {
        const right = d.kind === 'output'
        const fallback = { x: right ? COLUMN.output : COLUMN.input, y: rows[right ? 'right' : 'left']++ * ROW_HEIGHT }
        const pos = state.layout[d.id] ?? fallback
        return { id: d.id, x: pos.x, y: pos.y, el: deviceNode(d) }
      })
      const pos = state.layout[WINDOWS] ?? { x: COLUMN.input, y: -1.5 * ROW_HEIGHT }
      list.push({ id: WINDOWS, x: pos.x, y: pos.y, el: windowsNode(windowsOutput()) })
      graph.setNodes(list)
      Levels.apply()
    }

    // ---- cabos ----

    function cableState(r) {
      if (!r.active) return 'inactive'
      if (r.mute) return 'muted'
      const s = state.stats[routeId(r.from, r.to)]
      if (s && s.underruns + s.resyncs > 0) return 'warn'
      return 'live'
    }

    function renderCables() {
      const devices = byId()
      const list = state.routes
        .filter((r) => devices.has(r.from) && devices.has(r.to))
        .map((r) => {
          const id = routeId(r.from, r.to)
          const st = cableState(r)
          const selected = state.selection?.kind === 'cable' && state.selection.id === id
          return {
            id,
            source: r.from,
            target: r.to,
            cls: [st, selected && 'selected'],
            label: r.mute ? 'mudo' : Db.format(r.gain),
            labelCls: [st, selected && 'selected'],
            flow: st === 'live' || st === 'warn',
          }
        })
      const out = windowsOutput()
      if (out) {
        list.push({
          id: WINDOWS_CABLE,
          source: WINDOWS,
          target: out.id,
          cls: ['windows'],
          label: 'padrão do Windows',
          labelCls: ['windows'],
          movable: true,
        })
      }
      graph.setCables(list)
    }

    function canConnect(source, target) {
      const to = byId().get(target)
      if (source === WINDOWS) return (to?.kind === 'output' && !to.node) || isVirtualSpeaker(to)
      const from = byId().get(source)
      return !!from && !!to && from.kind === 'input' && to.kind === 'output'
    }

    function onConnect(source, target) {
      if (source === WINDOWS) return setWindowsOutput(target)
      const id = routeId(source, target)
      if (state.routes.some((r) => routeId(r.from, r.to) === id)) {
        select({ kind: 'cable', id }) // já existe: só seleciona
        return
      }
      const devices = byId()
      if (!devices.get(source)?.available || !devices.get(target)?.available) {
        Toast.show('Os dois dispositivos precisam estar conectados ao computador')
        return
      }
      act(async () => {
        await Api.connect(source, target)
        state.selection = { kind: 'cable', id }
      })
    }

    function setWindowsOutput(id) {
      if (id === windowsOutput()?.id) return
      if (!byId().get(id)?.available) {
        Toast.show('A saída precisa estar conectada ao computador')
        return
      }
      act(() => Api.setWindowsOutput(id))
    }

    // ---- painel lateral ----

    let panel = null // { key, el, update }

    function selectedRoute() {
      const sel = state.selection
      return sel?.kind === 'cable' ? state.routes.find((r) => routeId(r.from, r.to) === sel.id) : undefined
    }
    function selectedDevice() {
      const sel = state.selection
      return sel?.kind === 'device' ? byId().get(sel.id) : undefined
    }

    function renderSide() {
      const route = selectedRoute()
      const device = selectedDevice()
      const key = route ? `cable:${routeId(route.from, route.to)}` : device ? `device:${device.id}` : null
      if (!key) {
        panel = null
        side.hidden = true
        side.replaceChildren()
        return
      }
      if (panel?.key !== key) {
        panel = route
          ? {
              key,
              ...CablePanel.create({
                gain: (g) => withRoute((r) => Api.setGain(r.from, r.to, g)),
                mute: (m) => withRoute((r) => Api.setMute(r.from, r.to, m)),
                remove: removeSelectedCable,
              }),
            }
          : {
              key,
              ...DevicePanel.create({
                renamePair: (pair, name) => act(async () => warn(await Api.virtualRename(pair, name))),
                removePair: removePair,
                description: (text) => withDevice((id) => Api.setDescription(id, text)),
                delay: (ms) => withDevice((id) => Api.setDelay(id, ms)),
                mode: (mode) => withDevice((id) => Api.setMode(id, mode)),
                period: (ms) => withDevice((id) => Api.setPeriod(id, ms)),
              }),
            }
        side.replaceChildren(panel.el)
        side.hidden = false
      }
      if (route) panel.update(route, state.stats[routeId(route.from, route.to)], nameOf(route.from), nameOf(route.to))
      else panel.update(device, pairOf(device))
    }

    function withRoute(fn) {
      const r = selectedRoute()
      if (r) act(() => fn(r))
    }
    function withDevice(fn) {
      const d = selectedDevice()
      if (d) act(() => fn(d.id))
    }

    function removeSelectedCable() {
      const r = selectedRoute()
      if (!r) return
      state.selection = null
      act(() => Api.disconnect(r.from, r.to))
    }

    // ---- seleção, ações e carga ----

    function select(selection) {
      state.selection = selection
      render()
    }

    function render() {
      renderNodes()
      renderCables()
      renderSide()
    }

    async function reload() {
      const [devices, routes] = await Promise.all([Api.devices(), Api.routes()])
      state.devices = devices
      state.routes = routes
      // Se o item selecionado sumiu, fecha o painel.
      if (state.selection && !selectedRoute() && !selectedDevice()) state.selection = null
      render()
      if (!state.fitted && devices.length) {
        state.fitted = true
        graph.fit()
      }
    }

    /** Executa uma ação: mostra o erro, se houver, e recarrega o estado. */
    async function act(fn) {
      try {
        await fn()
      } catch (e) {
        Toast.show(e)
      }
      await reload().catch(Toast.show)
    }

    // ---- pares virtuais ----

    function warn(result) {
      const w = result?.warnings || []
      if (w.length) Toast.show(`O Windows não aceitou: ${w.join('; ')}`)
    }

    async function loadVirtual() {
      state.virtual = await Api.virtualDevices().catch(() => null)
      const v = state.virtual
      addPairButton.hidden = !v || v.pairs.length >= v.max
    }

    async function addPair() {
      const name = await promptDialog({
        title: 'Novo par virtual',
        message:
          'Cria um alto-falante e um microfone virtuais. Os programas veem os dois com este nome, ' +
          'por exemplo "Discord (audioroute)".',
        placeholder: 'Ex.: Discord, Jogo, Música',
        confirmLabel: 'Criar',
      })
      if (!name) return
      act(async () => {
        warn(await Api.virtualAdd(name))
        await loadVirtual()
      })
    }

    async function removePair(pair) {
      const ok = await confirmDialog({
        title: 'Remover par virtual?',
        message: 'O alto-falante e o microfone deste par somem do Windows e os cabos ligados a eles param.',
        confirmLabel: 'Remover',
      })
      if (!ok) return
      state.selection = null
      act(async () => {
        warn(await Api.virtualRemove(pair))
        await loadVirtual()
      })
    }

    function toggleLoopbacks() {
      state.showLoopbacks = !state.showLoopbacks
      loopbacksButton.classList.toggle('active', state.showLoopbacks)
      render()
    }

    let layoutTimer
    function onNodeMoved(id, pos) {
      state.layout[id] = pos
      clearTimeout(layoutTimer)
      layoutTimer = setTimeout(() => Api.saveLayout(state.layout).catch(Toast.show), 300)
      renderCables()
    }

    function onKey(e) {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement
      if (e.key === 'Delete' && !typing) removeSelectedCable()
      if (e.key === 'Escape') select(null)
    }

    // ---- rede ----

    function renderNetwork(net) {
      const peers = net?.peers || []
      // O endereço que se digita no participante quando ele não acha este computador sozinho.
      const here = net?.address ? `${net.address}${net.port && net.port !== 47801 ? `:${net.port}` : ''}` : null
      network.classList.toggle('on', peers.length > 0)
      network.textContent =
        (peers.length === 0 ? '🔗 Rede: nenhum participante' : `🔗 Rede: ${peers.length} ${peers.length === 1 ? 'computador' : 'computadores'}`) +
        (here ? ` · este computador: ${here}` : '')
      network.title = [
        ...peers.map((p) => `${p.name} (${p.address}): ${p.devices} dispositivos`),
        peers.length ? '' : 'Abra o audioroute em outro computador da mesma rede e escolha "Participante".',
        here ? `Se ele não achar este computador sozinho, digite lá: ${here}` : '',
      ]
        .filter(Boolean)
        .join('\n')
    }

    // ---- ciclo de vida ----

    const stopChanged = Api.onChanged(() => reload().catch(Toast.show))
    const stopStats = Api.onStats((list) => {
      state.stats = Object.fromEntries(list.map((s) => [routeId(s.from, s.to), s]))
      renderCables()
      if (selectedRoute()) renderSide()
    })
    const stopNetwork = Api.onNetwork(renderNetwork)
    Api.network().then(renderNetwork).catch(() => {})
    window.addEventListener('keydown', onKey)
    Levels.start()

    loadVirtual()
      .then(() => Api.loadLayout())
      .then((layout) => (state.layout = layout || {}))
      .catch(Toast.show)
      .then(() => reload())
      .catch(Toast.show)

    return () => {
      stopChanged()
      stopStats()
      stopNetwork()
      Levels.stop()
      window.removeEventListener('keydown', onKey)
    }
  },
}
