particle minecraft:sonic_boom ~ ~1 ~ 0 0 0 0 1 force
particle minecraft:explosion ~ ~0.5 ~ 1.5 0.3 1.5 0 3 force
execute as @a[distance=..3,gamemode=!spectator] run damage @s 7 minecraft:sonic_boom by @e[type=warden,tag=arnis.ds.here,limit=1]
