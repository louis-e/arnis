scoreboard players set #gravity arnis 0
# Players lose their modifiers on respawn, so they are checked every time; a repeat add just fails.
execute as @a run function arnis:gravity/apply
execute as @e[type=!player,tag=!arnis.gravity] run function arnis:gravity/apply
