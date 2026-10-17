// Medidores de nível.
//
// O backend manda o pico de cada janela de 50 ms; aqui o valor exibido sobe
// na hora e desce devagar (como um VU físico), para não piscar.
//
// Qualquer elemento com data-meter="<id>" vira barra (largura) e qualquer um
// com data-live="<id>" ganha a classe "live" quando há som.

const Levels = (() => {
  const FALL_PER_UPDATE = 0.82
  const LIVE_THRESHOLD = 0.003 // ~ -50 dB
  const values = {}
  let stop = null

  function apply() {
    for (const el of document.querySelectorAll('[data-meter]')) {
      el.style.width = `${Db.meterFraction(values[el.dataset.meter] || 0) * 100}%`
    }
    for (const el of document.querySelectorAll('[data-live]')) {
      el.classList.toggle('live', (values[el.dataset.live] || 0) > LIVE_THRESHOLD)
    }
  }

  return {
    start() {
      if (stop) stop()
      stop = Api.onLevels((peaks) => {
        for (const id of Object.keys(values)) {
          if (!(id in peaks)) values[id] *= FALL_PER_UPDATE
        }
        for (const [id, peak] of Object.entries(peaks)) {
          values[id] = Math.max(peak, (values[id] || 0) * FALL_PER_UPDATE)
        }
        apply()
      })
    },
    stop() {
      if (stop) stop()
      stop = null
    },
    /** Reaplica nos elementos recém-criados. */
    apply,
  }
})()
