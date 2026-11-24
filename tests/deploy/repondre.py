#!/usr/bin/env python3
"""Lance une commande dans un terminal simulé et répond à ses questions.

    python3 repondre.py <journal> <réponse>… -- <commande> <argument>…

Comme une personne au clavier : chaque réponse part quand une question est
affichée et que la commande attend (rien ne s'affiche plus depuis un
instant, et la dernière ligne finit par « : »). Tout ce qui s'affiche va dans
le journal. Sortie : le code de la commande ; 3 si elle pose une question de
trop (la question est dans le journal) ; 124 au bout de 15 minutes.

Pour les tests de deploy/lxc/install.sh, dont les questions lisent le
terminal (/dev/tty) et non l'entrée standard.
"""

import os
import pty
import re
import select
import signal
import sys
import time

COULEURS = re.compile(rb"\x1b\[[0-9;]*m")
LIMITE = 15 * 60
SILENCE = 0.5


def main() -> int:
    journal, *reste = sys.argv[1:]
    separation = reste.index("--")
    reponses = reste[:separation]
    commande = reste[separation + 1 :]

    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(commande[0], commande)

    fin = time.monotonic() + LIMITE
    derniere_ligne = b""
    dernier_affichage = time.monotonic()
    with open(journal, "wb") as log:

        def arreter(code: int, message: str) -> int:
            log.write(f"\n[repondre.py] {message}\n".encode())
            log.flush()
            os.kill(pid, signal.SIGTERM)
            os.waitpid(pid, 0)
            return code

        while True:
            if time.monotonic() > fin:
                return arreter(124, "trop long : arrêt")
            prets, _, _ = select.select([fd], [], [], 0.2)
            if prets:
                try:
                    donnees = os.read(fd, 4096)
                except OSError:
                    donnees = b""
                if not donnees:
                    break
                log.write(donnees)
                log.flush()
                derniere_ligne = (derniere_ligne + donnees).rsplit(b"\n", 1)[-1][-400:]
                dernier_affichage = time.monotonic()
                continue
            # Une question attend sa réponse ?
            question = COULEURS.sub(b"", derniere_ligne).rstrip(b" ")
            if question.endswith(b":") and time.monotonic() - dernier_affichage > SILENCE:
                if not reponses:
                    return arreter(3, "question sans réponse prévue : " + question.decode(errors="replace"))
                os.write(fd, reponses.pop(0).encode() + b"\n")
                derniere_ligne = b""

    _, statut = os.waitpid(pid, 0)
    if reponses:
        with open(journal, "ab") as log:
            log.write(f"\n[repondre.py] réponses non utilisées : {reponses}\n".encode())
    return os.waitstatus_to_exitcode(statut)


if __name__ == "__main__":
    sys.exit(main())
