scoreboard players set #kj arnis.ds 2
execute store result entity @s Pos[1] double 1 run scoreboard players get #kj_gy arnis.ds
data modify storage arnis:ds status set value "on the loose"
playsound minecraft:entity.warden.roar hostile @a[distance=..160] ~ ~20 ~ 6 0.6
