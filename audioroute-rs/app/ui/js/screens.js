// Telas simples: escolha de papel e participante.
// Cada tela é mount(root, ...) e devolve uma função que a desmonta.

/** Botão "Trocar papel": desliga o áudio e volta para a tela inicial. */
function switchRoleButton(onReset, confirmText) {
  return h(
    'button',
    {
      title: 'Voltar para a tela inicial',
      onClick: async () => {
        if (confirmText) {
          const ok = await confirmDialog({ title: 'Trocar de papel?', message: confirmText, confirmLabel: 'Trocar papel' })
          if (!ok) return
        }
        await Api.resetRole()
        onReset()
      },
    },
    '⇄ Trocar papel',
  )
}

const RoleSelect = {
  /** onChosen(role) quando o papel foi aceito pelo backend. */
  mount(root, onChosen) {
    const error = h('p', { class: 'error' })
    const busyLabel = h('em', { hidden: true }, 'Abrindo dispositivos…')

    async function choose(role) {
      for (const b of root.querySelectorAll('.card')) b.disabled = true
      busyLabel.hidden = role !== 'controller'
      error.textContent = ''
      try {
        await Api.setRole(role)
        onChosen(role)
      } catch (e) {
        // O backend pode já estar num papel (a página recarregou): segue para ele.
        const current = await Api.role().catch(() => '')
        if (current) onChosen(current)
        else error.textContent = String(e)
      } finally {
        for (const b of root.querySelectorAll('.card')) b.disabled = false
        busyLabel.hidden = true
      }
    }

    root.append(
      h(
        'main',
        { class: 'role-select' },
        h('h1', null, 'audioroute'),
        h('p', { class: 'subtitle' }, 'Como este computador vai participar?'),
        h(
          'div',
          { class: 'cards' },
          h(
            'button',
            { class: 'card controller', onClick: () => choose('controller') },
            h('span', { class: 'icon' }, '🎛️'),
            h('strong', null, 'Controlador principal'),
            h('span', { class: 'text' }, 'Monta as rotas entre os microfones e alto-falantes. Na rede, controla os outros computadores.'),
            busyLabel,
          ),
          h(
            'button',
            { class: 'card participant', onClick: () => choose('participant') },
            h('span', { class: 'icon' }, '🔗'),
            h('strong', null, 'Participante'),
            h('span', { class: 'text' }, 'Compartilha os dispositivos deste computador com o controlador principal da rede.'),
            h('em', { class: 'soon' }, 'Procura o controlador na rede local e conecta sozinho'),
          ),
        ),
        error,
      ),
    )
    return () => {}
  },
}

const Participant = {
  mount(root, onReset) {
    const inputs = h('ul')
    const outputs = h('ul')
    const error = h('p', { class: 'error' })
    const status = h('span', { class: 'status' })
    const item = (d, color) =>
      h(
        'li',
        null,
        h('span', null, d.name, d.isDefault && h('small', null, 'padrão')),
        h('div', { class: 'level' }, h('div', { class: 'fill', 'data-meter': d.id, style: { background: color } })),
      )

    root.append(
      h(
        'main',
        { class: 'participant' },
        h(
          'header',
          null,
          h('h1', null, 'Participante'),
          status,
          switchRoleButton(onReset),
        ),
        h(
          'p',
          { class: 'note' },
          'Estes dispositivos aparecem no canvas do controlador principal, que decide o que liga em quê. ' +
            'As barras mostram o som que passa em cada um agora, venha de qualquer programa.',
        ),
        h('h2', null, '🎤 Entradas'),
        inputs,
        h('h2', null, '🔊 Saídas'),
        outputs,
        error,
      ),
    )

    Api.devices()
      .then((list) => {
        inputs.append(...list.filter((d) => d.kind === 'input' && !d.loopback).map((d) => item(d, 'var(--input)')))
        outputs.append(...list.filter((d) => d.kind === 'output').map((d) => item(d, 'var(--output)')))
        Levels.apply()
      })
      .catch((e) => (error.textContent = String(e)))

    function renderStatus(net) {
      const s = net?.status
      status.className = 'status'
      if (!s || s.state === 'searching') {
        status.textContent = '⏳ Procurando o controlador principal na rede…'
      } else if (s.state === 'connected') {
        status.classList.add('ok')
        const using = s.streams ? ` · ${s.streams} em uso` : ''
        status.textContent = `✅ Conectado a ${s.controller} (${s.address})${using}`
      } else {
        status.classList.add('bad')
        status.textContent = `⚠️ ${s.error || 'erro na rede'}`
      }
    }
    renderStatus(null)
    Api.network().then(renderStatus).catch(() => {})
    const stopNetwork = Api.onNetwork(renderStatus)
    Levels.start()
    return () => {
      stopNetwork()
      Levels.stop()
    }
  },
}
