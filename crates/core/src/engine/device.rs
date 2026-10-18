//! O que roda dentro do callback de cada dispositivo.
//!
//! Regra de ouro do áudio em tempo real: a thread de áudio nunca trava,
//! nunca aloca e nunca libera memória. Por isso o grafo não é compartilhado
//! com locks: a thread de controle manda comandos por uma fila lock-free, e o
//! que sai da lista volta por outra fila para ser liberado lá.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use super::route::{CaptureEnd, End, RenderEnd};

/// Máximo de cabos por dispositivo (a lista é pré-alocada com esse tamanho).
pub(crate) const MAX_ROUTES: usize = 64;
const QUEUE: usize = 2 * MAX_ROUTES;

pub(crate) enum Command<E> {
    Add(Box<E>),
    Remove(u64),
}

/// Lado da thread de controle: envia comandos e recolhe o lixo.
pub(crate) struct Control<E> {
    pub commands: Producer<Command<E>>,
    pub garbage: Consumer<Box<E>>,
}

impl<E> Control<E> {
    /// Libera (aqui, fora da thread de áudio) o que o dispositivo devolveu.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

/// Estatísticas de um dispositivo: escritas pela thread de áudio, lidas
/// (e zeradas) pela de controle.
#[derive(Default)]
pub(crate) struct DeviceShared {
    pub calls: AtomicU64,
    pub busy_sum_ns: AtomicU64,
    pub busy_max_ns: AtomicU64,
    pub gap_min_ns: AtomicU64,
    pub gap_max_ns: AtomicU64,
    pub peak_bits: AtomicU32,
}

/// Lado da thread de áudio.
pub(crate) struct Processor<E> {
    commands: Consumer<Command<E>>,
    garbage: Producer<Box<E>>,
    ends: Vec<Box<E>>,
    shared: Arc<DeviceShared>,
    epoch: Instant,
    last_ns: u64,
}

pub(crate) fn channel<E>(shared: Arc<DeviceShared>, epoch: Instant) -> (Control<E>, Processor<E>) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(QUEUE);
    let (gb_tx, gb_rx) = RingBuffer::new(QUEUE);
    (
        Control { commands: cmd_tx, garbage: gb_rx },
        Processor {
            commands: cmd_rx,
            garbage: gb_tx,
            ends: Vec::with_capacity(MAX_ROUTES),
            shared,
            epoch,
            last_ns: 0,
        },
    )
}

impl<E: End> Processor<E> {
    /// Aplica os comandos pendentes. Sem alocação: a lista tem capacidade
    /// reservada e os itens removidos voltam pela fila de lixo.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Add(end) if self.ends.len() < MAX_ROUTES => self.ends.push(end),
                Command::Add(end) => {
                    let _ = self.garbage.push(end);
                }
                Command::Remove(id) => {
                    if let Some(i) = self.ends.iter().position(|e| e.route_id() == id) {
                        let _ = self.garbage.push(self.ends.swap_remove(i));
                    }
                }
            }
        }
    }

    fn begin(&mut self) -> u64 {
        let now = self.epoch.elapsed().as_nanos() as u64;
        if self.last_ns != 0 {
            let gap = now - self.last_ns;
            let min = self.shared.gap_min_ns.load(Relaxed);
            if min == 0 || gap < min {
                self.shared.gap_min_ns.store(gap, Relaxed);
            }
            if gap > self.shared.gap_max_ns.load(Relaxed) {
                self.shared.gap_max_ns.store(gap, Relaxed);
            }
        }
        self.last_ns = now;
        self.apply_commands();
        now
    }

    fn end(&self, start_ns: u64, samples: &[f32]) {
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        if peak > f32::from_bits(self.shared.peak_bits.load(Relaxed)) {
            self.shared.peak_bits.store(peak.to_bits(), Relaxed);
        }
        let busy = (self.epoch.elapsed().as_nanos() as u64).saturating_sub(start_ns);
        self.shared.busy_sum_ns.fetch_add(busy, Relaxed);
        if busy > self.shared.busy_max_ns.load(Relaxed) {
            self.shared.busy_max_ns.store(busy, Relaxed);
        }
        self.shared.calls.fetch_add(1, Relaxed);
    }
}

impl Processor<CaptureEnd> {
    /// Callback da entrada: copia o bloco para cada cabo que sai dela (1 → N).
    pub fn capture(&mut self, samples: &[f32]) {
        let now = self.begin();
        for end in &mut self.ends {
            end.write(samples, now);
        }
        self.end(now, samples);
    }
}

impl Processor<RenderEnd> {
    /// Callback da saída: soma os cabos que chegam nela (N → 1) e limita a [-1, 1].
    pub fn render(&mut self, out: &mut [f32]) {
        let now = self.begin();
        out.fill(0.0);
        for end in &mut self.ends {
            end.mix_into(out, now);
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        self.end(now, out);
    }
}
