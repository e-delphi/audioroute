// Ponto de entrada: mostra a tela do papel escolhido (ou a escolha de papel).

;(() => {
  const root = document.getElementById('app')
  let unmount = null

  function show(role) {
    if (unmount) unmount()
    root.replaceChildren()
    if (role === 'controller') unmount = Controller.mount(root, () => show(''))
    else if (role === 'participant') unmount = Participant.mount(root, () => show(''))
    else unmount = RoleSelect.mount(root, show)
  }

  Api.role()
    .then(show)
    .catch(() => show(''))
})()
