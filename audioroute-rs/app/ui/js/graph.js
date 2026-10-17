// Canvas de nós e cabos.
//
// * fundo de pontos numa grade de 24 px; os nós sempre encaixam nela;
// * arrastar o fundo move a vista, a roda do mouse dá zoom no ponto do cursor;
// * arrastar um nó o move; arrastar de um conector de saída (.handle.out) até
//   um nó com entrada (.handle.in) cria um cabo;
// * um cabo "movable" tem uma bolinha na ponta que pode ser levada para outro nó.
//
// Quem usa fornece o elemento de cada nó (com os conectores dentro) e a lista
// de cabos; este módulo só posiciona, desenha e avisa o que o usuário fez.

const Graph = (() => {
  const GRID = 24
  const MIN_ZOOM = 0.3
  const MAX_ZOOM = 2
  /** Movimento até o qual um pressionar-soltar ainda conta como clique. */
  const CLICK_SLOP = 4
  /** Distância (px na tela) em que soltar um cabo perto de um nó ainda liga. */
  const DROP_RADIUS = 40
  const SVG = 'http://www.w3.org/2000/svg'

  const snap = (v) => Math.round(v / GRID) * GRID

  function svg(tag, attrs) {
    const el = document.createElementNS(SVG, tag)
    for (const [k, v] of Object.entries(attrs)) if (v !== undefined && v !== false) el.setAttribute(k, v)
    return el
  }

  /** Curva como a do Vue Flow: sai na horizontal e chega na horizontal. */
  function bezier(sx, sy, tx, ty) {
    const dx = tx - sx
    const off = dx >= 0 ? 0.5 * dx : 0.25 * 25 * Math.sqrt(-dx)
    const c1 = sx + off
    const c2 = tx - off
    return {
      d: `M${sx},${sy} C${c1},${sy} ${c2},${ty} ${tx},${ty}`,
      mx: (sx + 3 * c1 + 3 * c2 + tx) / 8,
      my: (sy + ty) / 2,
    }
  }

  /**
   * callbacks: canConnect(source, target), onConnect(source, target),
   * onNodeClick(id), onCableClick(id), onPaneClick(), onNodeMoved(id, {x, y}).
   */
  function create(root, callbacks) {
    root.classList.add('graph')
    const cablesSvg = svg('svg', { class: 'graph-cables' })
    const nodesLayer = h('div', { class: 'graph-nodes' })
    const labelsLayer = h('div', { class: 'graph-labels' })
    const viewport = h('div', { class: 'graph-viewport' })
    viewport.append(cablesSvg, nodesLayer, labelsLayer)
    const controls = h(
      'div',
      { class: 'graph-controls' },
      h('button', { title: 'Aproximar', onClick: () => zoomBy(1.2) }, '+'),
      h('button', { title: 'Afastar', onClick: () => zoomBy(1 / 1.2) }, '−'),
      h('button', { title: 'Enquadrar tudo', onClick: () => fit() }, '⤢'),
    )
    root.append(viewport, controls)

    const view = { x: 0, y: 0, k: 1 }
    /** id → { el, x, y } */
    const nodes = new Map()
    let cables = []
    let drag = null

    // ---- vista (pan e zoom) ----

    function applyView() {
      viewport.style.transform = `translate(${view.x}px, ${view.y}px) scale(${view.k})`
      const step = GRID * view.k
      root.style.backgroundSize = `${step}px ${step}px`
      // O ponto fica no centro do ladrilho: desloca meio passo para cair na grade.
      root.style.backgroundPosition = `${view.x - step / 2}px ${view.y - step / 2}px`
      root.style.setProperty('--dot', `${Math.max(0.6, 0.85 * view.k)}px`)
    }

    function toWorld(clientX, clientY) {
      const r = root.getBoundingClientRect()
      return { x: (clientX - r.left - view.x) / view.k, y: (clientY - r.top - view.y) / view.k }
    }

    function zoomAt(px, py, k) {
      k = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, k))
      const wx = (px - view.x) / view.k
      const wy = (py - view.y) / view.k
      view.k = k
      view.x = px - wx * k
      view.y = py - wy * k
      applyView()
    }

    function zoomBy(factor) {
      zoomAt(root.clientWidth / 2, root.clientHeight / 2, view.k * factor)
    }

    /** Enquadra todos os nós (zoom no máximo 100%). */
    function fit(padding = 0.12) {
      if (!nodes.size) return
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity
      for (const n of nodes.values()) {
        minX = Math.min(minX, n.x)
        minY = Math.min(minY, n.y)
        maxX = Math.max(maxX, n.x + n.el.offsetWidth)
        maxY = Math.max(maxY, n.y + n.el.offsetHeight)
      }
      const w = maxX - minX
      const hgt = maxY - minY
      const W = root.clientWidth
      const H = root.clientHeight
      const k = Math.min(1, Math.max(MIN_ZOOM, Math.min(W / (w * (1 + 2 * padding)), H / (hgt * (1 + 2 * padding)))))
      view.k = k
      view.x = W / 2 - (minX + w / 2) * k
      view.y = H / 2 - (minY + hgt / 2) * k
      applyView()
    }

    root.addEventListener(
      'wheel',
      (e) => {
        e.preventDefault()
        const r = root.getBoundingClientRect()
        zoomAt(e.clientX - r.left, e.clientY - r.top, view.k * Math.exp(-e.deltaY * 0.0015))
      },
      { passive: false },
    )

    // ---- nós ----

    /** list: [{ id, x, y, el }]. Posições são encaixadas na grade. */
    function setNodes(list) {
      nodes.clear()
      nodesLayer.replaceChildren()
      for (const n of list) {
        const wrap = h('div', { class: 'graph-node', 'data-node': n.id }, n.el)
        const node = { el: wrap, x: snap(n.x), y: snap(n.y) }
        place(node)
        nodes.set(n.id, node)
        nodesLayer.append(wrap)
      }
      drawCables()
    }

    function place(node) {
      node.el.style.transform = `translate(${node.x}px, ${node.y}px)`
    }

    /** Ponto do conector: saída à direita, entrada à esquerda, no meio da altura. */
    function port(id, side) {
      const n = nodes.get(id)
      if (!n) return null
      const y = n.y + n.el.offsetHeight / 2
      return side === 'out' ? { x: n.x + n.el.offsetWidth, y } : { x: n.x, y }
    }

    // ---- cabos ----

    /**
     * list: [{ id, source, target, cls: [], label?, labelCls?: [], flow?: bool,
     * movable?: bool }]
     */
    function setCables(list) {
      cables = list
      drawCables()
    }

    function drawCables() {
      cablesSvg.replaceChildren()
      labelsLayer.replaceChildren()
      for (const c of cables) {
        const a = port(c.source, 'out')
        const b = port(c.target, 'in')
        if (!a || !b) continue
        const p = bezier(a.x, a.y, b.x, b.y)
        const g = svg('g', { 'data-cable': c.id, class: drag && drag.hide === c.id ? 'cable-dragging' : undefined })
        g.append(svg('path', { d: p.d, class: 'cable-hit' }))
        g.append(svg('path', { d: p.d, class: ['cable', ...(c.cls || [])].join(' ') }))
        if (c.flow) g.append(svg('path', { d: p.d, class: 'cable-flow' }))
        if (c.movable) g.append(svg('circle', { cx: b.x, cy: b.y, r: 6, class: 'cable-anchor', 'data-anchor': c.id }))
        cablesSvg.append(g)
        if (c.label && !(drag && drag.hide === c.id)) {
          const label = h('div', { class: ['graph-label', ...(c.labelCls || [])] }, c.label)
          label.style.left = `${p.mx}px`
          label.style.top = `${p.my}px`
          labelsLayer.append(label)
        }
      }
      if (drag && drag.kind === 'connect') {
        const a = port(drag.source, 'out')
        const b = drag.target ? port(drag.target, 'in') : drag.point
        if (a && b) cablesSvg.append(svg('path', { d: bezier(a.x, a.y, b.x, b.y).d, class: 'cable temp' }))
      }
    }

    // ---- interação ----

    function startConnect(source, e, hide) {
      drag = { kind: 'connect', source, hide, point: toWorld(e.clientX, e.clientY), target: null }
      for (const [id, n] of nodes) n.el.classList.toggle('can-drop', callbacks.canConnect(source, id))
      drawCables()
    }

    /** Nó que aceita o cabo sob o cursor (ou com a entrada a menos de DROP_RADIUS). */
    function dropTarget(source, point) {
      let best = null
      let bestDist = DROP_RADIUS / view.k
      for (const [id, n] of nodes) {
        if (!callbacks.canConnect(source, id)) continue
        const inside = point.x >= n.x && point.x <= n.x + n.el.offsetWidth && point.y >= n.y && point.y <= n.y + n.el.offsetHeight
        if (inside) return id
        const p = port(id, 'in')
        const d = Math.hypot(p.x - point.x, p.y - point.y)
        if (d < bestDist) {
          best = id
          bestDist = d
        }
      }
      return best
    }

    root.addEventListener('pointerdown', (e) => {
      if (e.button !== 0 || e.target.closest('.graph-controls')) return
      const start = { x: e.clientX, y: e.clientY }
      const handle = e.target.closest('.handle.out')
      const anchor = e.target.closest('[data-anchor]')
      const nodeEl = e.target.closest('[data-node]')
      const cableEl = e.target.closest('[data-cable]')

      if (anchor) {
        const c = cables.find((x) => x.id === anchor.dataset.anchor)
        if (c) startConnect(c.source, e, c.id)
      } else if (handle && nodeEl) {
        startConnect(nodeEl.dataset.node, e, null)
      } else if (nodeEl) {
        const node = nodes.get(nodeEl.dataset.node)
        const w = toWorld(e.clientX, e.clientY)
        drag = { kind: 'node', id: nodeEl.dataset.node, start, dx: w.x - node.x, dy: w.y - node.y, moved: false }
      } else if (cableEl) {
        drag = { kind: 'cable', id: cableEl.dataset.cable, start }
      } else {
        drag = { kind: 'pan', start, vx: view.x, vy: view.y, moved: false }
        root.classList.add('panning')
      }
      root.setPointerCapture(e.pointerId)
      e.preventDefault()
    })

    root.addEventListener('pointermove', (e) => {
      if (!drag) return
      const moved = drag.start && Math.hypot(e.clientX - drag.start.x, e.clientY - drag.start.y) > CLICK_SLOP
      if (drag.kind === 'pan') {
        drag.moved ||= moved
        view.x = drag.vx + e.clientX - drag.start.x
        view.y = drag.vy + e.clientY - drag.start.y
        applyView()
      } else if (drag.kind === 'node') {
        if (!drag.moved && !moved) return
        drag.moved = true
        const node = nodes.get(drag.id)
        const w = toWorld(e.clientX, e.clientY)
        const x = snap(w.x - drag.dx)
        const y = snap(w.y - drag.dy)
        node.el.classList.add('dragging')
        if (x !== node.x || y !== node.y) {
          node.x = x
          node.y = y
          place(node)
          drawCables()
        }
      } else if (drag.kind === 'connect') {
        drag.point = toWorld(e.clientX, e.clientY)
        const target = dropTarget(drag.source, drag.point)
        if (target !== drag.target) {
          if (drag.target) nodes.get(drag.target)?.el.classList.remove('drop-target')
          if (target) nodes.get(target).el.classList.add('drop-target')
          drag.target = target
        }
        drawCables()
      }
    })

    function endDrag(e, cancelled) {
      if (!drag) return
      const d = drag
      drag = null
      root.classList.remove('panning')
      if (root.hasPointerCapture(e.pointerId)) root.releasePointerCapture(e.pointerId)
      if (d.kind === 'pan' && !d.moved && !cancelled) callbacks.onPaneClick()
      if (d.kind === 'cable' && !cancelled) callbacks.onCableClick(d.id)
      if (d.kind === 'node') {
        const node = nodes.get(d.id)
        node?.el.classList.remove('dragging')
        if (d.moved) callbacks.onNodeMoved(d.id, { x: node.x, y: node.y })
        else if (!cancelled) callbacks.onNodeClick(d.id)
      }
      if (d.kind === 'connect') {
        for (const n of nodes.values()) n.el.classList.remove('can-drop', 'drop-target')
        if (d.target && !cancelled) callbacks.onConnect(d.source, d.target)
        drawCables()
      }
    }
    root.addEventListener('pointerup', (e) => endDrag(e, false))
    root.addEventListener('pointercancel', (e) => endDrag(e, true))

    applyView()
    return { setNodes, setCables, fit }
  }

  return { create, GRID }
})()
