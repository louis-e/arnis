scoreboard players set #kj arnis.ds 3
data modify storage arnis:ds status set value "going back underground"
execute as @e[type=warden,tag=arnis.ds.kaiju] at @s run playsound minecraft:entity.warden.dig hostile @a[distance=..128] ~ ~ ~ 6 0.5
