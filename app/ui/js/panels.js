// Painéis laterais: cabo selecionado e dispositivo selecionado.
//
// Cada painel é montado uma vez por seleção e depois só atualizado (update),
// para não perder o slider ou o texto que o usuário está mexendo.

/** Algum campo deste painel está em edição agora? */
const editing = (el) => document.activeElement === el

const CablePanel = {
  MIN_DB: -40,
  MAX_DB: 12,

  /** actions: gain(g), mute(m), remove(). */
  create(actions) {
    const { MIN_DB, MAX_DB } = CablePanel
    const clampDb = (v) => Math.min(MAX_DB, Math.max(MIN_DB, isFinite(v) ? v : MIN_DB))
    const gainOf = (db) => (db <= MIN_DB ? 0 : Db.fromDb(db))

    const from = h('span')
    const to = h('span')
    const value = h('strong', { class: 'value' })
    const slider = h('input', { type: 'range', min: MIN_DB, max: MAX_DB, step: 0.5 })
    const mute = h('button')
    const stats = h('dl', { class: 'stats' })
    let route = null

    // Envia no máximo a cada 80 ms enquanto o slider é arrastado.
    let timer
    const send = () => {
      value.textContent = Db.format(gainOf(+slider.value))
      clearTimeout(timer)
      timer = setTimeout(() => actions.gain(gainOf(+slider.value)), 80)
    }
    slider.addEventListener('input', send)

    const el = h(
      'aside',
      { class: 'panel' },
      h('h2', null, 'Cabo'),
      h('p', { class: 'path' }, from, h('span', { class: 'arrow' }, '→'), to),
      h('label', { class: 'field' }, h('span', null, 'Volume ', value), slider),
      h(
        'div',
        { class: 'row' },
        h('button', { onClick: () => ((slider.value = 0), send()) }, '0 dB'),
        mute,
      ),
      stats,
      h('button', { class: 'danger', style: { marginTop: '8px' }, onClick: () => actions.remove() }, 'Remover cabo'),
      h('p', { class: 'hint' }, 'Atalho: Delete'),
    )
    mute.addEventListener('click', () => {
      mute.blur()
      actions.mute(!route.mute)
    })

    function update(r, s, fromName, toName) {
      route = r
      from.textContent = fromName
      to.textContent = toName
      if (!editing(slider)) {
        slider.value = clampDb(Db.toDb(r.gain))
        value.textContent = Db.format(r.gain)
      }
      mute.textContent = r.mute ? '🔇 Mudo' : '🔈 Mutar'
      mute.classList.toggle('active', r.mute)

      let health
      if (!r.active) health = [`Inativo: ${r.error || 'um dos dispositivos está desconectado'}`, 'bad']
      else if (!s) health = ['Iniciando…', '']
      else {
        const problems = s.underruns + s.overruns + s.resyncs
        health = problems ? [`${problems} falhas desde o início`, 'warn'] : ['Sem falhas', 'ok']
      }
      const rows = [h('dt', null, 'Estado'), h('dd', { class: health[1] }, health[0])]
      if (s && r.active) {
        rows.push(
          h('dt', null, 'Latência'),
          h('dd', { title: 'Estimativa: driver de entrada + buffer do cabo + driver de saída' }, `~${s.latencyMs.toFixed(1)} ms`),
          h('dt', null, 'Buffer do cabo'),
          h('dd', null, `${s.levelMs.toFixed(1)} ms (alvo ${s.targetMs.toFixed(1)})`),
          h('dt', null, 'Correção de drift'),
          h('dd', null, `${s.driftPpm >= 0 ? '+' : ''}${s.driftPpm.toFixed(1)} ppm`),
        )
      }
      stats.replaceChildren(...rows)
    }

    return { el, update }
  },
}

const DevicePanel = {
  MAX_DELAY: 5000,
  /** Períodos oferecidos; o backend arredonda para o que o hardware aceitar. */
  PERIODS: [1, 2, 2.67, 3, 4, 5, 8, 10, 20],

  /**
   * actions: description(text), delay(ms), mode(mode), period(ms | null),
   * renamePair(pair, name), removePair(pair).
   */
  create(actions) {
    const { MAX_DELAY, PERIODS } = DevicePanel
    let device = null

    const name = h('p', { class: 'name' })
    const meta = h('p', { class: 'meta' })
    const shared = h('button', { onClick: () => (shared.blur(), actions.mode('shared')) }, 'Compartilhado')
    const exclusive = h('button', { onClick: () => (exclusive.blur(), actions.mode('exclusive')) }, 'Exclusivo')
    const modeHint = h('small')
    const autoOption = h('option', { value: 'auto' })
    const period = h('select', null, autoOption, PERIODS.map((p) => h('option', { value: p }, `${p} ms`)))
    period.addEventListener('change', () => {
      period.blur()
      actions.period(period.value === 'auto' ? null : Number(period.value))
    })
    const running = h('div')
    const delayRange = h('input', { type: 'range', min: 0, max: 500, step: 1 })
    const delayNumber = h('input', { type: 'number', min: 0, max: MAX_DELAY })
    const delayHint = h('small')
    const description = h('textarea', { rows: 3, placeholder: 'Ex.: microfone da mesa da sala' })
    const id = h('p', { class: 'id' })
    // Par virtual (só nos dispositivos do driver).
    const pairName = h('input', { type: 'text', placeholder: 'Nome do par (ex.: Discord)' })
    const pairTitle = h('span')
    let pair = null
    pairName.addEventListener('change', () => {
      const name = pairName.value.trim()
      if (pair && name !== pair.name) actions.renamePair(pair.pair, name)
    })
    const pairSection = h(
      'div',
      { class: 'field' },
      pairTitle,
      pairName,
      h('small', null, 'Nome que os programas veem: "<nome> (audioroute)", no alto-falante e no microfone do par.'),
      h('button', { class: 'danger', onClick: () => pair && actions.removePair(pair.pair) }, 'Remover par'),
    )
    const modeField = h('div', { class: 'field' })
    const periodField = h('label', { class: 'field' })

    const saveDelay = (input) => {
      const ms = Math.min(MAX_DELAY, Math.max(0, Math.round(Number(input.value) || 0)))
      delayRange.value = delayNumber.value = ms
      if (ms !== device.delayMs) actions.delay(ms)
    }
    delayRange.addEventListener('input', () => (delayNumber.value = delayRange.value))
    delayRange.addEventListener('change', () => saveDelay(delayRange))
    delayNumber.addEventListener('change', () => saveDelay(delayNumber))
    description.addEventListener('blur', () => {
      const text = description.value.trim()
      if (text !== device.description) actions.description(text)
    })

    const el = h(
      'aside',
      { class: 'panel' },
      h('h2', null, 'Dispositivo'),
      name,
      meta,
      pairSection,
      modeField,
      periodField,
      running,
      h(
        'label',
        { class: 'field' },
        h('span', null, 'Atraso de sincronização'),
        h('div', { class: 'delay' }, delayRange, delayNumber, h('span', null, 'ms')),
        delayHint,
      ),
      h('label', { class: 'field' }, h('span', null, 'Descrição'), description),
      id,
    )

    modeField.append(h('span', null, 'Modo'), h('div', { class: 'row' }, shared, exclusive), modeHint)
    periodField.append(
      h('span', null, 'Período'),
      period,
      h('small', null, 'Menor = menos latência, mas exige mais do driver. Se o som falhar, aumente.'),
    )

    function update(d, virtualPair = null) {
      device = d
      pair = virtualPair
      pairSection.hidden = !pair
      // Os virtuais não têm modo nem período: andam em blocos de 2 ms.
      modeField.hidden = !!pair
      periodField.hidden = !!pair
      if (pair) {
        pairTitle.textContent = `Par virtual ${pair.pair + 1}`
        if (!editing(pairName)) pairName.value = pair.name
      }
      const kind = d.loopback ? 'Loopback (entrada)' : d.kind === 'output' ? 'Saída' : 'Entrada'
      name.textContent = d.name
      meta.replaceChildren(
        kind,
        d.node ? ` · no computador ${d.node}` : '',
        d.isDefault ? ' · padrão do sistema' : '',
        d.available ? '' : h('span', { class: 'off' }, ' · desconectado'),
      )

      shared.classList.toggle('active', d.mode === 'shared')
      exclusive.classList.toggle('active', d.mode === 'exclusive')
      exclusive.disabled = d.loopback
      exclusive.title = d.loopback ? 'Loopback só funciona em modo compartilhado' : ''
      if (d.mode === 'exclusive' && d.isDefault && d.kind === 'output') {
        modeHint.className = 'warn'
        modeHint.textContent =
          'Esta é a saída padrão do Windows: com um cabo ligado em exclusivo, o som dos outros programas para. Leve o cabo do Windows para outra saída antes.'
      } else if (d.mode === 'exclusive') {
        modeHint.className = 'warn'
        modeHint.textContent =
          'Menor latência, mas nenhum outro programa consegue usar este dispositivo enquanto o cabo estiver ligado.'
      } else {
        modeHint.className = ''
        modeHint.textContent = 'Outros programas continuam usando o dispositivo normalmente.'
      }

      autoOption.textContent = d.mode === 'exclusive' ? 'Automático (5 ms)' : 'Automático (menor do Windows)'
      if (!editing(period)) period.value = d.periodMs === null ? 'auto' : String(d.periodMs)

      running.replaceChildren()
      if (d.stream) {
        running.className = 'running'
        running.append(
          h('span', null, 'Rodando agora'),
          h('strong', null, `${d.stream.periodMs.toFixed(2)} ms`),
          h('small', null, d.stream.format),
        )
      } else {
        running.className = 'idle'
        running.textContent = 'Sem cabos ligados: o dispositivo não está aberto.'
      }

      if (!editing(delayRange) && !editing(delayNumber)) delayRange.value = delayNumber.value = d.delayMs
      delayHint.textContent =
        d.kind === 'output' ? 'Atrasa tudo o que toca nesta saída.' : 'Atrasa esta entrada em todos os cabos dela.'
      if (!editing(description)) description.value = d.description
      id.textContent = `ID ${d.id}`
      id.title = d.id
    }

    return { el, update }
  },
}
