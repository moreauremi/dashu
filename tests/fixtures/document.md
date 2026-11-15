---
title: Flux principaux du réseau
lang: fr-FR
---

Fiche de révision : les flux qui traversent le réseau de l'école, du poste de travail jusqu'à Internet. Le texte est **en gras** par endroits, *en italique* ailleurs, avec un peu de `code dans le texte` et [un lien vers le site](https://dashu.dev).

# Vue d'ensemble

Chaque flux part d'un VLAN, passe par le cœur de réseau, puis par le pare-feu. L'œil doit trouver vite le titre, la liste, le tableau.

## Les VLAN

- VLAN 10 : postes des élèves
- VLAN 20 : professeurs
    - salles de cours
    - salle des profs
- VLAN 99 : administration des équipements

1. Le poste demande une adresse (DHCP).
2. Le commutateur étiquette la trame.
3. Le routeur fait passer d'un VLAN à l'autre.

- [x] Plan d'adressage relu
- [ ] Règles du pare-feu à vérifier

### Débits mesurés

| Flux | Source | Destination | Débit |
|:-----|:-------|:------------|------:|
| Web | VLAN 10 | Internet | 80 Mb/s |
| Sauvegardes | Serveurs | NAS | 450 Mb/s |
| Téléphonie | VLAN 30 | IPBX | 2 Mb/s |

> Un trunk transporte plusieurs VLAN sur un même lien ; chaque trame garde
> son étiquette 802.1Q jusqu'au commutateur suivant.

## Configuration

```
interface GigabitEthernet0/1
 switchport mode trunk
 switchport trunk allowed vlan 10,20,99
```

---

![Schéma des flux](schema.png)

Une dernière phrase avec une note de bas de page[^1] et des guillemets « à la française ».

[^1]: Relevé du 15 novembre 2026.
