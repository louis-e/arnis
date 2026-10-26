scoreboard players add #try arnis.ds 1
execute summon marker run function arnis:disaster/sinkhole/aim
execute if score #found arnis.ds matches 0 if score #try arnis.ds matches ..6 run function arnis:disaster/sinkhole/attempt
