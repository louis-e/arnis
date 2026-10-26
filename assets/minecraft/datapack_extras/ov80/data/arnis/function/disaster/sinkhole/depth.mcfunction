execute store result storage arnis:ds sink.dd int 1 run scoreboard players get @s arnis.ds.d
scoreboard players set #cl arnis.ds 33
scoreboard players operation #cl arnis.ds -= @s arnis.ds.d
execute store result storage arnis:ds sink.cl int 1 run scoreboard players get #cl arnis.ds
