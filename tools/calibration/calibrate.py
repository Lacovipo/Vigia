"""Calibración del eval de Vigia contra 4 oráculos (SF/Obsidian/Berserk/Caissa).

Fase 1: genera partidas rápidas Vigia-vs-Vigia y muestrea posiciones.
Fase 2: consulta `eval` estático de Vigia y de cada oráculo por posición.
Salida: CSV con fen, ply, phase, vigia, sf, obsidian, berserk, caissa.
"""

import csv
import os
import pathlib
import re
import subprocess
import sys

import chess
import chess.engine

# Relativa al propio script: tools/calibration/ cuelga de la raíz del
# repositorio, así que el binario está siempre dos niveles más arriba. La ruta
# absoluta que había aquí solo valía en el equipo donde se escribió, y esta
# carpeta se sincroniza entre varios. VIGIA_EXE la sobrescribe si hace falta.
VIGIA = os.environ.get(
    "VIGIA_EXE",
    str(pathlib.Path(__file__).resolve().parents[2] / "target" / "release" / "vigia.exe"),
)
# Los oráculos sí viven fuera del repositorio y no hay ruta relativa posible:
# se deja la carpeta donde el usuario los tiene y AJEENG la cambia de un tirón.
# Las versiones van en el nombre a propósito: recalibrar contra otra versión de
# un oráculo no da lo mismo, y conviene que se note al leer el fichero.
AJEENG = pathlib.Path(os.environ.get("AJEENG", r"C:\AjeEng"))
ORACLES = {
    "sf": str(AJEENG / "Stockfish" / "18" / "stockfish_18_64_ja_avx512.exe"),
    "obsidian": str(AJEENG / "Obsidian" / "Obsidian 16.15 SE" / "Obsidian_dev16.15SE_BMI2.exe"),
    "berserk": str(AJEENG / "Berserk" / "berserk_14_64_ja_avx512.exe"),
    "caissa": str(AJEENG / "Caissa" / "caissa_125_64_ja_avx512.exe"),
}
OUT_CSV = sys.argv[1] if len(sys.argv) > 1 else "calib.csv"
GAMES_PER_OPENING = 4
MOVETIME = 0.03  # s por jugada en la generación
MAX_PLIES = 140

# El mismo librillo de 8 líneas del harness selfplay del proyecto.
OPENINGS = [
    [],
    ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"],
    ["e2e4", "c7c5"],
    ["e2e4", "e7e6"],
    ["d2d4", "d7d5", "c2c4"],
    ["d2d4", "g8f6", "c2c4", "g7g6"],
    ["c2c4"],
    ["g1f3", "d7d5", "c2c4"],
]

PHASE_WEIGHTS = {chess.KNIGHT: 1, chess.BISHOP: 1, chess.ROOK: 2, chess.QUEEN: 4}


def game_phase(board: chess.Board) -> int:
    phase = 0
    for piece, w in PHASE_WEIGHTS.items():
        phase += w * (len(board.pieces(piece, chess.WHITE)) + len(board.pieces(piece, chess.BLACK)))
    return min(phase, 24)


def generate_positions():
    """Juega partidas rápidas y devuelve una lista de (fen, ply, phase)."""
    positions = []
    seen = set()
    engine = chess.engine.SimpleEngine.popen_uci(VIGIA)
    try:
        for opening in OPENINGS:
            for _ in range(GAMES_PER_OPENING):
                board = chess.Board()
                for mv in opening:
                    board.push_uci(mv)
                ply = len(opening)
                while ply < MAX_PLIES and not board.is_game_over(claim_draw=True):
                    result = engine.play(board, chess.engine.Limit(time=MOVETIME))
                    if result.move is None:
                        break
                    board.push(result.move)
                    ply += 1
                    # Muestreo: desde la jugada 5, plies alternos, sin jaques
                    # (el eval estático no significa nada en jaque y Vigia ni
                    # lo intenta), y sin repetir posiciones ya vistas.
                    if ply >= 10 and ply % 2 == 0 and not board.is_check():
                        key = board.board_fen() + (" w" if board.turn else " b")
                        if key not in seen:
                            seen.add(key)
                            positions.append((board.fen(), ply, game_phase(board)))
    finally:
        engine.quit()
    return positions


class EvalClient:
    """Cliente UCI mínimo para el comando no estándar `eval`."""

    def __init__(self, path):
        self.p = subprocess.Popen(
            [path], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )
        self._cmd("uci", "uciok")

    def _cmd(self, cmd, until):
        self.p.stdin.write(cmd + "\n")
        self.p.stdin.flush()
        lines = []
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError(f"EOF esperando '{until}'")
            line = line.strip()
            lines.append(line)
            if line == until:
                return lines

    def eval_lines(self, fen):
        self.p.stdin.write(f"position fen {fen}\neval\nisready\n")
        self.p.stdin.flush()
        lines = []
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("EOF esperando readyok")
            line = line.strip()
            if line == "readyok":
                return lines
            lines.append(line)

    def quit(self):
        try:
            self.p.stdin.write("quit\n")
            self.p.stdin.flush()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def parse_eval(name, lines, black_to_move):
    """Devuelve cp desde el punto de vista blanco, o None si no hay eval."""
    text = "\n".join(lines)
    if name == "vigia":
        m = re.search(r"Evaluation: (-?\d+)", text)
        return int(m.group(1)) if m else None
    if name == "sf":
        if "in check" in text:
            return None
        m = re.search(r"Final evaluation\s+([+-]?\d+\.\d+)", text)
        return int(round(float(m.group(1)) * 100)) if m else None
    if name == "obsidian":
        m = re.search(r"Evaluation: (-?\d+)", text)
        return int(m.group(1)) if m else None
    if name == "berserk":
        m = re.search(r"Final Score: (-?\d+)cp", text)
        return int(m.group(1)) if m else None
    if name == "caissa":
        vals = [l for l in lines if re.fullmatch(r"-?\d+", l)]
        if not vals:
            return None
        v = int(vals[-1])
        return -v if black_to_move else v
    return None


def main():
    print("Generando partidas...", flush=True)
    positions = generate_positions()
    print(f"{len(positions)} posiciones muestreadas", flush=True)

    clients = {"vigia": EvalClient(VIGIA)}
    for name, path in ORACLES.items():
        clients[name] = EvalClient(path)

    rows = []
    for i, (fen, ply, phase) in enumerate(positions):
        black_to_move = " b " in fen
        row = {"fen": fen, "ply": ply, "phase": phase}
        ok = True
        for name, client in clients.items():
            try:
                score = parse_eval(name, client.eval_lines(fen), black_to_move)
            except RuntimeError as e:
                print(f"{name} murió en {fen}: {e}", flush=True)
                score = None
                ok = False
            row[name] = score
            if score is None:
                ok = False
        if ok:
            rows.append(row)
        if (i + 1) % 100 == 0:
            print(f"  {i + 1}/{len(positions)} evaluadas", flush=True)

    for client in clients.values():
        client.quit()

    with open(OUT_CSV, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=["fen", "ply", "phase", "vigia", "sf", "obsidian", "berserk", "caissa"])
        writer.writeheader()
        writer.writerows(rows)
    print(f"{len(rows)} filas escritas en {OUT_CSV}", flush=True)


if __name__ == "__main__":
    main()
