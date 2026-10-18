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

/**
 * Aviso do Firewall do Windows: aparece só quando ele está barrando o
 * audioroute na rede atual, com um botão para liberar.
 */
function firewallBanner(consequence) {
  const text = h('span')
  const allow = h('button', { class: 'primary' }, 'Liberar no firewall')
  const again = h('button', null, 'Verificar de novo')
  const banner = h('div', { class: 'firewall', hidden: true }, h('span', { class: 'icon' }, '🛡️'), text, allow, again)

  function show(s) {
    banner.hidden = !s || s.ok
    if (s && !s.ok) text.textContent = `Firewall do Windows: ${s.problem}. ${consequence}`
  }
  async function check() {
    show(await Api.firewall().catch(() => null))
  }
  allow.addEventListener('click', async () => {
    allow.disabled = true
    allow.textContent = 'Aguardando a confirmação do Windows…'
    try {
      show(await Api.allowFirewall())
    } catch (e) {
      Toast.show(e)
    } finally {
      allow.disabled = false
      allow.textContent = 'Liberar no firewall'
    }
  })
  again.addEventListener('click', check)
  check()
  return banner
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
    const netError = h('small', { class: 'net-error' })
    const address = h('input', { type: 'text', placeholder: 'Automático (procura na rede)', spellcheck: 'false' })
    const connectButton = h('button', { class: 'primary', onClick: () => setAddress(address.value) }, 'Conectar')
    const autoButton = h('button', { onClick: () => ((address.value = ''), setAddress(null)) }, 'Automático')
    address.addEventListener('keydown', (e) => e.key === 'Enter' && setAddress(address.value))

    async function setAddress(value) {
      try {
        renderStatus(await Api.setControllerAddress(value && value.trim() ? value.trim() : null))
      } catch (e) {
        Toast.show(e)
      }
    }
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
          'section',
          { class: 'connect' },
          h('label', { class: 'field' }, h('span', null, 'Controlador principal'), h('div', { class: 'row' }, address, connectButton, autoButton)),
          h(
            'small',
            null,
            'Se ele não aparecer sozinho (algumas redes bloqueiam a busca automática), digite o endereço que aparece ',
            'no topo da tela do controlador, como 192.168.0.10.',
          ),
          netError,
        ),
        firewallBanner('O controlador não vai conseguir mandar o áudio para este computador.'),
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

    let addressLoaded = false
    function renderStatus(net) {
      const s = net?.status
      // O endereço salvo entra no campo uma vez; depois o campo é do usuário.
      if (s && !addressLoaded) {
        addressLoaded = true
        address.value = s.manual || ''
      }
      netError.textContent = s && s.state !== 'connected' && s.error ? `Última tentativa: ${s.error}` : ''
      status.className = 'status'
      if (!s || s.state === 'searching') {
        status.textContent = '⏳ Procurando o controlador principal na rede…'
      } else if (s.state === 'connecting') {
        status.textContent = `⏳ Conectando a ${s.manual}…`
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
