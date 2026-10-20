// Utilidades da interface: montar elementos, dB, aviso de erro e confirmação.

/**
 * Cria um elemento. `attrs`: class (texto ou lista, valores falsos ignorados),
 * style (objeto), on<evento> (função), demais viram atributos (false/null
 * ignorados). Filhos: texto, elementos, listas ou valores falsos (ignorados).
 */
function h(tag, attrs, ...children) {
  const el = document.createElement(tag)
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === false || value === null || value === undefined) continue
    if (key === 'class') {
      el.className = [].concat(value).filter(Boolean).join(' ')
    } else if (key === 'style') {
      Object.assign(el.style, value)
    } else if (key.startsWith('on') && typeof value === 'function') {
      el.addEventListener(key.slice(2).toLowerCase(), value)
    } else if (key === 'value') {
      el.value = value
    } else {
      el.setAttribute(key, value === true ? '' : value)
    }
  }
  append(el, children)
  return el
}

function append(el, children) {
  for (const child of children) {
    if (child === false || child === null || child === undefined) continue
    if (Array.isArray(child)) append(el, child)
    else el.append(child instanceof Node ? child : String(child))
  }
}

const Db = {
  /** Ganho linear → decibéis (0 → -Infinity). */
  toDb: (gain) => (gain <= 0 ? -Infinity : 20 * Math.log10(gain)),
  /** Decibéis → ganho linear. */
  fromDb: (db) => Math.pow(10, db / 20),
  format(gain) {
    const db = Db.toDb(gain)
    if (!isFinite(db)) return '−∞ dB'
    const sign = db > 0.05 ? '+' : db < -0.05 ? '−' : ''
    return `${sign}${Math.abs(db).toFixed(1)} dB`
  },
  /** Pico (0–1) → largura do medidor, em dB de -60 a 0 (como o ouvido percebe). */
  meterFraction(peak) {
    if (peak <= 0) return 0
    const db = 20 * Math.log10(peak)
    return Math.min(1, Math.max(0, (db + 60) / 60))
  },
}

const routeId = (from, to) => `${from}->${to}`

/** Aviso de erro no rodapé, some sozinho em 4 s. */
const Toast = (() => {
  let el = null
  let timer
  return {
    show(error) {
      if (!el) {
        el = h('div', { class: 'toast', onClick: () => el.remove() })
      }
      el.textContent = String(error)
      document.body.append(el)
      clearTimeout(timer)
      timer = setTimeout(() => el.remove(), 4000)
    },
  }
})()

/**
 * Caixa que pede um texto. Enter confirma; Esc ou clique fora cancela.
 * Resolve o texto (sem espaços nas pontas) ou null.
 */
function promptDialog({ title, message, value = '', placeholder = '', confirmLabel = 'OK', cancelLabel = 'Cancelar' }) {
  return new Promise((resolve) => {
    const close = (answer) => {
      window.removeEventListener('keydown', onKey, true)
      backdrop.remove()
      resolve(answer)
    }
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        close(null)
      } else if (e.key === 'Enter') {
        e.stopPropagation()
        close(input.value.trim())
      }
    }
    const input = h('input', { type: 'text', value, placeholder, spellcheck: 'false' })
    const backdrop = h(
      'div',
      { class: 'backdrop', onClick: (e) => e.target === backdrop && close(null) },
      h(
        'div',
        { class: 'dialog', role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
        h('h2', null, title),
        message && h('p', null, message),
        h('div', { class: 'dialog-input' }, input),
        h(
          'div',
          { class: 'actions' },
          h('button', { onClick: () => close(null) }, cancelLabel),
          h('button', { class: 'primary', onClick: () => close(input.value.trim()) }, confirmLabel),
        ),
      ),
    )
    window.addEventListener('keydown', onKey, true)
    document.body.append(backdrop)
    input.focus()
    input.select()
  })
}

/**
 * Caixa de confirmação no visual do app. Enter confirma; Esc ou clique fora
 * cancela. Resolve true/false.
 */
function confirmDialog({ title, message, confirmLabel = 'Continuar', cancelLabel = 'Cancelar' }) {
  return new Promise((resolve) => {
    const close = (answer) => {
      window.removeEventListener('keydown', onKey, true)
      backdrop.remove()
      resolve(answer)
    }
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        close(false)
      }
    }
    const ok = h('button', { class: 'primary', onClick: () => close(true) }, confirmLabel)
    const backdrop = h(
      'div',
      { class: 'backdrop', onClick: (e) => e.target === backdrop && close(false) },
      h(
        'div',
        { class: 'dialog', role: 'alertdialog', 'aria-modal': 'true', 'aria-label': title },
        h('h2', null, title),
        h('p', null, message),
        h('div', { class: 'actions' }, h('button', { onClick: () => close(false) }, cancelLabel), ok),
      ),
    )
    window.addEventListener('keydown', onKey, true)
    document.body.append(backdrop)
    ok.focus()
  })
}
