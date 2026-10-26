execute unless entity @a[distance=7..36,gamemode=!spectator] run return 0
scoreboard players set #kj_cd arnis.ds 0
kill @e[type=marker,tag=arnis.ds.kj_mark]
execute as @r[distance=7..36,gamemode=!spectator] at @s summon marker run function arnis:disaster/kaiju/mark
scoreboard players set #kj_boom arnis.ds 34
playsound minecraft:entity.warden.sonic_charge hostile @a[distance=..64] ~ ~20 ~ 4 0.8
