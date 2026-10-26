playsound minecraft:entity.warden.step hostile @a[distance=..96] ~ ~ ~ 4 0.5
particle minecraft:block{block_state:"minecraft:dirt"} ~ ~0.2 ~ 4 0.2 4 0 30 force
execute as @a[distance=..40,gamemode=!spectator] at @s run rotate @s ~ ~1.2
execute positioned ~-6 ~-2 ~-6 as @a[dx=12,dy=8,dz=12,gamemode=!spectator] run damage @s 6 minecraft:mob_attack by @e[type=warden,tag=arnis.ds.here,limit=1]
