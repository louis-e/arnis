bossbar set arnis:boss players @a
execute store result bossbar arnis:boss value run data get entity @e[tag=arnis.ob.boss,limit=1] Health
scoreboard players add #boss_t arnis.ob 1
execute if score #boss_t arnis.ob matches 12.. as @e[tag=arnis.ob.boss,limit=1] at @s run function arnis:outbreak/boss/minions
